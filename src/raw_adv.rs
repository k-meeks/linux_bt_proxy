use std::collections::HashMap;
use zbus::zvariant::{Dict, OwnedValue};

const BASE_UUID_SUFFIX: [u8; 12] = [
    0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB,
];

enum ShortUuid {
    Bit16(u16),
    Bit32(u32),
    Bit128([u8; 16]),
}

/// Parses a canonical (dashed) 128-bit UUID string and, if it follows the
/// Bluetooth SIG base UUID pattern, shrinks it to its 16- or 32-bit form --
/// matching how such UUIDs are actually encoded over the air.
fn parse_uuid(uuid_str: &str) -> Option<ShortUuid> {
    let hex: String = uuid_str.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }

    if bytes[4..16] == BASE_UUID_SUFFIX[..] {
        if bytes[0] == 0 && bytes[1] == 0 {
            Some(ShortUuid::Bit16(u16::from_be_bytes([bytes[2], bytes[3]])))
        } else {
            Some(ShortUuid::Bit32(u32::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3],
            ])))
        }
    } else {
        Some(ShortUuid::Bit128(bytes))
    }
}

fn push_ad(out: &mut Vec<u8>, ad_type: u8, payload: &[u8]) {
    out.push((payload.len() + 1) as u8);
    out.push(ad_type);
    out.extend_from_slice(payload);
}

fn push_uuid_list_ad(out: &mut Vec<u8>, uuids: &[String]) {
    let mut bit16 = Vec::new();
    let mut bit32 = Vec::new();
    let mut bit128 = Vec::new();

    for uuid in uuids {
        match parse_uuid(uuid) {
            Some(ShortUuid::Bit16(id)) => bit16.extend_from_slice(&id.to_le_bytes()),
            Some(ShortUuid::Bit32(id)) => bit32.extend_from_slice(&id.to_le_bytes()),
            Some(ShortUuid::Bit128(bytes)) => {
                let mut le = bytes;
                le.reverse();
                bit128.extend_from_slice(&le);
            }
            None => {}
        }
    }

    // Complete List of 16/32/128-bit Service UUIDs
    if !bit16.is_empty() {
        push_ad(out, 0x03, &bit16);
    }
    if !bit32.is_empty() {
        push_ad(out, 0x05, &bit32);
    }
    if !bit128.is_empty() {
        push_ad(out, 0x07, &bit128);
    }
}

fn push_service_data_ad(out: &mut Vec<u8>, value_opt: Option<&OwnedValue>) {
    let Some(v) = value_opt else {
        return;
    };
    let Ok(dict) = Dict::try_from(v.to_owned()) else {
        return;
    };

    for (k, v) in dict {
        let uuid_str = k.downcast::<String>().ok();
        let data: Result<Vec<u8>, _> = v.downcast();

        let (Some(uuid_str), Ok(data)) = (uuid_str, data) else {
            continue;
        };

        let mut payload = Vec::new();
        match parse_uuid(&uuid_str) {
            Some(ShortUuid::Bit16(id)) => {
                payload.extend_from_slice(&id.to_le_bytes());
                payload.extend_from_slice(&data);
                push_ad(out, 0x16, &payload); // Service Data - 16-bit UUID
            }
            Some(ShortUuid::Bit32(id)) => {
                payload.extend_from_slice(&id.to_le_bytes());
                payload.extend_from_slice(&data);
                push_ad(out, 0x20, &payload); // Service Data - 32-bit UUID
            }
            Some(ShortUuid::Bit128(bytes)) => {
                let mut le = bytes;
                le.reverse();
                payload.extend_from_slice(&le);
                payload.extend_from_slice(&data);
                push_ad(out, 0x21, &payload); // Service Data - 128-bit UUID
            }
            None => {}
        }
    }
}

fn push_manufacturer_data_ad(out: &mut Vec<u8>, value_opt: Option<&OwnedValue>) {
    let Some(v) = value_opt else {
        return;
    };
    let Ok(dict) = Dict::try_from(v.to_owned()) else {
        return;
    };

    for (k, v) in dict {
        let company_id = k.downcast::<u16>().ok();
        let data: Result<Vec<u8>, _> = v.downcast();

        let (Some(company_id), Ok(data)) = (company_id, data) else {
            continue;
        };

        let mut payload = company_id.to_le_bytes().to_vec();
        payload.extend_from_slice(&data);
        push_ad(out, 0xFF, &payload); // Manufacturer Specific Data
    }
}

/// Reconstructs a raw BLE advertising-data (AD structure) byte stream from
/// BlueZ's already-parsed Device1 properties.
///
/// BlueZ does not expose the original over-the-air bytes of an advertisement
/// (it coalesces/parses them into Device1 properties), so this is a
/// best-effort re-encoding: the payload bytes of service/manufacturer data
/// are byte-exact (BlueZ passes those through verbatim), but framing details
/// BlueZ doesn't surface (e.g. the AD Flags byte) are omitted.
pub fn build_raw_advertisement_data(props: &HashMap<String, OwnedValue>) -> Vec<u8> {
    let mut out = Vec::new();

    if let Some(name) = props
        .get("Name")
        .and_then(|v| v.downcast_ref::<String>().ok())
    {
        push_ad(&mut out, 0x09, name.as_bytes()); // Complete Local Name
    }

    if let Some(uuids) = props
        .get("UUIDs")
        .cloned()
        .and_then(|v| Vec::<String>::try_from(v).ok())
    {
        push_uuid_list_ad(&mut out, &uuids);
    }

    push_service_data_ad(&mut out, props.get("ServiceData"));
    push_manufacturer_data_ad(&mut out, props.get("ManufacturerData"));

    if let Some(tx_power) = props
        .get("TxPower")
        .and_then(|v| v.downcast_ref::<i16>().ok())
    {
        if let Ok(tx_power_i8) = i8::try_from(tx_power) {
            push_ad(&mut out, 0x0A, &[tx_power_i8 as u8]); // TX Power Level
        }
    }

    out
}
