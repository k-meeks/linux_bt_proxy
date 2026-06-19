use futures_util::stream::StreamExt;
use log::{debug, error, info, warn};
use sd_notify::NotifyState;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::broadcast::Sender;

use zbus::fdo::PropertiesProxy;
use zbus::match_rule::MatchRule;
use zbus::names::InterfaceName;
use zbus::zvariant::{Dict, ObjectPath, OwnedValue};
use zbus::{message::Type, Connection, MessageStream, Proxy};

use crate::api::api::{
    BluetoothLEAdvertisementResponse, BluetoothLERawAdvertisement, BluetoothServiceData,
};
use crate::raw_adv::build_raw_advertisement_data;
use crate::utils::parse_mac;

#[derive(Debug, Clone)]
pub struct BleAdvertisement {
    pub legacy: BluetoothLEAdvertisementResponse,
    pub raw: BluetoothLERawAdvertisement,
}

/// Looks up the MAC address of a BlueZ adapter via D-Bus (org.bluez.Adapter1.Address),
/// rather than opening a raw HCI socket. This avoids requiring CAP_NET_RAW.
pub async fn get_adapter_mac(conn: &Connection, adapter_index: u16) -> zbus::Result<[u8; 6]> {
    let adapter_path = format!("/org/bluez/hci{adapter_index}");
    let proxy = Proxy::new(
        conn,
        "org.bluez",
        ObjectPath::try_from(adapter_path.as_str())?,
        "org.bluez.Adapter1",
    )
    .await?;

    let address: String = proxy.get_property("Address").await?;
    parse_mac(&address).map_err(zbus::Error::Failure)
}

/// Runs the BlueZ advertisement listener under supervision: if it ever
/// returns (cleanly or with an error), this logs loudly, reports status to
/// systemd (if running under it), and restarts it with capped exponential
/// backoff instead of leaving the proxy permanently without BLE data.
pub async fn run_supervised(adapter_index: u16, tx: Sender<BleAdvertisement>) {
    const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    // If a run lasted at least this long before failing, treat the next
    // failure as a fresh problem rather than continuing to ramp up backoff.
    const HEALTHY_RUN_THRESHOLD: Duration = Duration::from_secs(60);

    let mut backoff = INITIAL_BACKOFF;

    loop {
        let started = Instant::now();
        let result = run_bluez_advertisement_listener(adapter_index, tx.clone()).await;
        let ran_for = started.elapsed();

        match result {
            Ok(()) => warn!("BLE advertisement listener exited unexpectedly after {ran_for:?}"),
            Err(e) => error!("BLE advertisement listener failed after {ran_for:?}: {e}"),
        }

        if ran_for >= HEALTHY_RUN_THRESHOLD {
            backoff = INITIAL_BACKOFF;
        }

        let status = format!("BLE listener down, retrying in {}s", backoff.as_secs());
        warn!("{status}");
        let _ = sd_notify::notify(&[NotifyState::Status(&status)]);

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

pub async fn run_bluez_advertisement_listener(
    adapter_index: u16,
    tx: Sender<BleAdvertisement>,
) -> zbus::Result<()> {
    let conn = Connection::system().await?;
    let adapter_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .arg(0, "org.bluez.Adapter1")?
        .build();

    let props_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .arg(0, "org.bluez.Device1")?
        .build();

    let iface_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.DBus.ObjectManager")?
        .member("InterfacesAdded")?
        .build();

    let mut adapter_stream = MessageStream::for_match_rule(adapter_rule, &conn, None).await?;
    let mut props_stream = MessageStream::for_match_rule(props_rule, &conn, None).await?;
    let mut iface_stream = MessageStream::for_match_rule(iface_rule, &conn, None).await?;

    // All streams ready: now start discovery
    try_start_discovery(&conn, adapter_index).await?;

    loop {
        tokio::select! {
            maybe_msg = adapter_stream.next() => {
                if let Some(Ok(msg)) = maybe_msg {
                    let body = msg.body();
                    let (interface, changed, _invalidated): (String, HashMap<String, OwnedValue>, Vec<String>) =
                        body.deserialize()?;

                    if interface == "org.bluez.Adapter1" {
                        if let Some(value) = changed.get("Discovering") {
                            if let Ok(is_discovering) = value.downcast_ref::<bool>() {
                                if !is_discovering {
                                    info!("Discovery was turned off — restarting discovery.");
                                    try_start_discovery(&conn, adapter_index).await?;
                                }
                            }
                        }
                    }
                }
            }

            maybe_msg = props_stream.next() => {
                if let Some(msg) = maybe_msg.transpose()? {
                    let device_path = msg.header().path().map(|p| p.to_string());
                    if let Some(path) = device_path {
                        match get_device_properties(&conn, &path).await {
                            Ok(props) => {
                                debug!("Changed properties for device {path}");
                                if log::log_enabled!(log::Level::Debug) {
                                    print_props(&props);
                                }
                                match build_advertisement(&props) {
                                    Some(msg) => {
                                        if let Err(e) = tx.send(msg) {
                                            warn!("Failed to send advertisement response: {e}");
                                        }
                                    }
                                    None => {
                                        warn!("Failed to build advertisement response for {path}");
                                    }
                                };
                                }
                            Err(e) => {
                                warn!("Failed to fetch properties for {path}: {e}");
                            }
                        }
                    }
                }
            }

            maybe_msg = iface_stream.next() => {
                if let Some(msg) = maybe_msg.transpose()? {
                    let body = msg.body();
                    let (path, interfaces): (
                        ObjectPath<'_>,
                        HashMap<String, HashMap<String, OwnedValue>>
                    ) = body.deserialize()?;

                    debug!("InterfacesAdded at path: {path}");
                    match interfaces.get("org.bluez.Device1") {
                        Some(props) => {
                            debug!("New properties for device {path}");
                            if log::log_enabled!(log::Level::Debug) {
                                print_props(props);
                            }
                            match build_advertisement(props) {
                                Some(msg) => {
                                    if let Err(e) = tx.send(msg) {
                                        warn!("Failed to send advertisement response: {e}");
                                    }
                                }
                                None => {
                                    warn!("Failed to build advertisement response for {path}");
                                }
                            };
                        }
                        _ => {
                            debug!("Failed to fetch properties for {path}");
                        }
                    }
                }
            }
        } // select
    } // loop
      // Note: This function will run indefinitely, listening for advertisements.
}

fn build_advertisement(props: &HashMap<String, OwnedValue>) -> Option<BleAdvertisement> {
    let mac_opt = props
        .get("Address")
        .and_then(|v| v.downcast_ref::<String>().ok());

    let address_type = props
        .get("AddressType")
        .and_then(|v| v.downcast_ref::<String>().ok())
        .map(|s| if s == "random" { 1 } else { 0 })
        .unwrap_or(0);

    let rssi = props
        .get("RSSI")
        .and_then(|v| v.downcast_ref::<i16>().ok().map(|x| x as i32))
        .unwrap_or(-127);

    let name = props
        .get("Name")
        .and_then(|v| v.downcast_ref::<String>().ok())
        .map_or_else(Vec::new, |s| s.into_bytes());

    let service_uuids = props
        .get("UUIDs")
        .cloned()
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default();

    let service_data = extract_service_data(props.get("ServiceData"), true).unwrap_or_default();

    let manufacturer_data =
        extract_service_data(props.get("ManufacturerData"), false).unwrap_or_default();

    let mac_str = mac_opt?;
    let address = parse_ble_address(&mac_str);

    let legacy = BluetoothLEAdvertisementResponse {
        address,
        address_type,
        name,
        rssi,
        service_uuids,
        service_data,
        manufacturer_data,
        ..Default::default()
    };

    let raw = BluetoothLERawAdvertisement {
        address,
        address_type,
        rssi,
        data: build_raw_advertisement_data(props),
        ..Default::default()
    };

    Some(BleAdvertisement { legacy, raw })
}

async fn get_device_properties(
    conn: &Connection,
    path: &str,
) -> zbus::Result<HashMap<String, OwnedValue>> {
    let proxy = PropertiesProxy::new(conn, "org.bluez", ObjectPath::try_from(path)?).await?;

    let props: HashMap<String, OwnedValue> = proxy
        .get_all(InterfaceName::try_from("org.bluez.Device1")?)
        .await?;

    Ok(props)
}

fn print_props(props: &HashMap<String, OwnedValue>) {
    for (key, value) in props {
        debug!("  {key} => {value:?}");
    }
}

async fn try_start_discovery(conn: &Connection, adapter_index: u16) -> zbus::Result<()> {
    let adapter_path = format!("/org/bluez/hci{adapter_index}");
    let proxy = Proxy::new(
        conn,
        "org.bluez",
        ObjectPath::try_from(adapter_path.as_str())?,
        "org.bluez.Adapter1",
    )
    .await?;

    proxy
        .call_method("SetDiscoveryFilter", &(HashMap::<&str, OwnedValue>::new()))
        .await?;
    match proxy.call_method("StartDiscovery", &()).await {
        Ok(_) => info!("Discovery started"),
        Err(zbus::Error::MethodError(ref name, _, _))
            if name.as_str() == "org.bluez.Error.InProgress" =>
        {
            warn!("Discovery already in progress");
        }
        Err(e) => return Err(e),
    }

    Ok(())
}

fn parse_ble_address(address: &str) -> u64 {
    address.split(':').fold(0, |acc, part| {
        (acc << 8) | u8::from_str_radix(part, 16).unwrap_or(0) as u64
    })
}

fn extract_service_data(
    value_opt: Option<&OwnedValue>,
    is_service: bool,
) -> Result<Vec<BluetoothServiceData>, Box<dyn std::error::Error>> {
    // If no value present, return empty list
    let Some(v) = value_opt else {
        return Ok(Vec::new());
    };

    let dict = Dict::try_from(v.to_owned())?;

    let mut entries = Vec::new();

    for (k, v) in dict {
        // Extract UUID
        let uuid = if is_service {
            k.downcast::<String>().ok()
        } else {
            // Company id must be hex (aioesphomeapi parses this field with int(uuid, 16)),
            // not decimal -- otherwise the manufacturer id is silently corrupted.
            k.downcast::<u16>().ok().map(|id| format!("{id:04x}"))
        };

        let data: Result<Vec<u8>, _> = v.downcast();

        // If both present, create entry
        if let (Some(uuid), Ok(data)) = (uuid, data) {
            entries.push(BluetoothServiceData {
                uuid,
                data,
                ..Default::default()
            });
        }
    }

    Ok(entries)
}
