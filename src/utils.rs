use anyhow::Result;

pub fn format_mac(mac: &[u8], sep: &str) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(sep)
}

pub fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err("Invalid MAC format: expected 6 hex bytes separated by ':'".to_string());
    }

    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).map_err(|_| format!("Invalid hex byte: '{part}'"))?;
    }
    Ok(mac)
}
