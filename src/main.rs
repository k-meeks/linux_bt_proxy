mod api;
mod ble;
mod context;
mod handlers;
mod mdns;
mod proto;
mod raw_adv;
mod server;
mod utils;

use clap::Parser;
use gethostname::gethostname;
use log::{info, warn};
use mac_address::get_mac_address;
use sd_notify::NotifyState;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::context::ProxyContext;
use crate::utils::parse_mac;

fn default_hostname() -> String {
    gethostname().to_string_lossy().into_owned()
}

#[derive(Parser, Debug)]
#[command(name = "linux_bt_proxy")]
#[command(about = "Bluetooth Proxy Daemon for ESPHome", long_about = None)]
struct Cli {
    /// Bluetooth adapter index (e.g. 0 for hci0)
    #[arg(short = 'a', long, default_value_t = 0)]
    hci: u16,

    /// TCP listen address (default: [::]:6053)
    #[arg(short, long, default_value = "[::]:6053")]
    listen: SocketAddr,

    /// Hostname to advertise (default: system hostname)
    #[arg(long, default_value_t = default_hostname())]
    hostname: String,

    /// MAC address for mDNS
    #[arg(short, long, value_parser = parse_mac)]
    mac: Option<[u8; 6]>,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    let dbus_conn = match zbus::Connection::system().await {
        Ok(conn) => conn,
        Err(e) => {
            log::error!("Failed to connect to system D-Bus: {e}");
            log::error!("Fatal: Is dbus-daemon running?");
            std::process::exit(1);
        }
    };

    // Look up the adapter's MAC over D-Bus (org.bluez.Adapter1.Address) rather than
    // a raw HCI socket, so the proxy doesn't need CAP_NET_RAW. Retry a few times in
    // case bluetoothd is still starting up.
    const MAC_LOOKUP_ATTEMPTS: u32 = 5;
    let mut bt_mac = None;
    for attempt in 1..=MAC_LOOKUP_ATTEMPTS {
        match ble::get_adapter_mac(&dbus_conn, cli.hci).await {
            Ok(mac) => {
                bt_mac = Some(mac);
                break;
            }
            Err(e) if attempt < MAC_LOOKUP_ATTEMPTS => {
                log::warn!(
                    "Could not read MAC for hci{} (attempt {attempt}/{MAC_LOOKUP_ATTEMPTS}): {e}; retrying...",
                    cli.hci
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => {
                log::error!("Failed to get Bluetooth MAC for hci{}: {e}", cli.hci);
                log::error!(
                    "Fatal: Cannot access Bluetooth adapter hci{}. Check 'bluetoothctl list' and that bluetoothd is running.",
                    cli.hci
                );
                std::process::exit(1);
            }
        }
    }
    let bt_mac = bt_mac.expect("loop above always sets bt_mac or exits");
    drop(dbus_conn);

    let mac: [u8; 6] = match cli.mac {
        Some(mac) => mac,
        None => match get_mac_address() {
            Ok(Some(mac)) => mac.bytes(),
            Ok(None) => {
                log::warn!("System has no available MAC address.");
                log::error!("Fatal: No MAC address provided via CLI or available on system.");
                std::process::exit(1);
            }
            Err(e) => {
                log::error!("Error while getting MAC address: {e}");
                log::error!("Fatal: Could not determine MAC address.");
                std::process::exit(1);
            }
        },
    };

    let ctx = Arc::new(ProxyContext {
        hostname: cli.hostname,
        port: cli.listen.port(),
        net_mac: mac,
        bt_mac,
        build_time: env!("BUILD_TIME"),
        version: env!("CARGO_PKG_VERSION"),
    });

    let (tx, rx) = broadcast::channel(100);

    // Supervised: logs and retries with backoff on failure instead of dying silently.
    tokio::spawn(ble::run_supervised(cli.hci, tx.clone()));
    info!("Listening for ble advertisements on hci{}", cli.hci);

    mdns::start_mdns(ctx.clone()).unwrap_or_else(|e| {
        warn!("Critical error: failed to register mDNS service: {e}");
        std::process::exit(1);
    });
    info!("mDNS service registered");

    let listener = match TcpListener::bind(cli.listen).await {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("Failed to bind TCP listener on {}: {e}", cli.listen);
            std::process::exit(1);
        }
    };
    info!("Listening on {}", cli.listen);

    // Tell systemd (if we're running under it, Type=notify) that startup is complete.
    let _ = sd_notify::notify(&[
        NotifyState::Ready,
        NotifyState::Status("Serving ESPHome API"),
    ]);

    // If systemd configured a watchdog timeout (WatchdogSec=), ping it well within
    // that interval so a fully wedged process gets noticed and restarted.
    if let Some(timeout) = sd_notify::watchdog_enabled() {
        let ping_interval = timeout / 2;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(ping_interval);
            loop {
                ticker.tick().await;
                let _ = sd_notify::notify(&[NotifyState::Watchdog]);
            }
        });
    }

    if let Err(e) = server::run_tcp_server(ctx.clone(), listener, rx).await {
        log::error!("TCP server error: {e}");
        log::error!("Fatal: TCP server failed to start or crashed.");
        std::process::exit(1);
    }

    Ok(())
}
