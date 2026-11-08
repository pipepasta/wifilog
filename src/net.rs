//! Looks up the Wi-Fi interface, its gateway (router), and the router's MAC address.
//!
//! macOS redacts the SSID / BSSID, so networks are told apart by the router's MAC address instead.

use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct Router {
    pub ip: String,
    pub mac: Option<String>,
}

/// Extracts the Wi-Fi device name (e.g. `en0`) from `networksetup -listallhardwareports` output.
pub fn parse_wifi_device(output: &str) -> Option<String> {
    let mut lines = output.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "Hardware Port: Wi-Fi" {
            return lines
                .next()?
                .trim()
                .strip_prefix("Device: ")
                .map(|d| d.trim().to_string());
        }
    }
    None
}

/// Extracts the gateway IP from `ipconfig getoption <dev> router` output. Empty when not connected.
pub fn parse_router_ip(output: &str) -> Option<String> {
    let ip = output.trim();
    (!ip.is_empty()).then(|| ip.to_string())
}

/// Extracts the MAC address from `arp -n <ip>` output, normalized to the `00:1a:2b:03:04:05` form.
pub fn parse_arp_mac(output: &str) -> Option<String> {
    let raw = output.split(" at ").nth(1)?.split_whitespace().next()?;
    let octets: Vec<&str> = raw.split(':').collect();
    let valid = octets.len() == 6
        && octets
            .iter()
            .all(|o| (1..=2).contains(&o.len()) && o.chars().all(|c| c.is_ascii_hexdigit()));
    if !valid {
        return None;
    }
    Some(
        octets
            .iter()
            .map(|o| format!("{:0>2}", o.to_ascii_lowercase()))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The Wi-Fi device name.
pub fn wifi_device() -> Option<String> {
    parse_wifi_device(&run("networksetup", &["-listallhardwareports"])?)
}

/// The Wi-Fi gateway and its MAC address. `None` when Wi-Fi is not connected.
pub fn current_router(device: &str) -> Option<Router> {
    let ip = parse_router_ip(&run("ipconfig", &["getoption", device, "router"])?)?;
    let mac = run("arp", &["-n", &ip]).and_then(|o| parse_arp_mac(&o));
    Some(Router { ip, mac })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_device_from_networksetup() {
        let out = "Hardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: N/A\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: 12:34:56:78:9a:bc\n";
        assert_eq!(parse_wifi_device(out), Some("en0".into()));
    }

    #[test]
    fn router_ip_from_ipconfig() {
        assert_eq!(parse_router_ip("192.168.11.1\n"), Some("192.168.11.1".into()));
        assert_eq!(parse_router_ip(""), None); // Wi-Fi not connected
    }

    #[test]
    fn arp_mac_is_normalized() {
        let out = "? (192.168.11.1) at 0:1a:2b:3:4:5 on en0 ifscope [ethernet]\n";
        assert_eq!(parse_arp_mac(out), Some("00:1a:2b:03:04:05".into()));
    }

    #[test]
    fn arp_incomplete_is_none() {
        assert_eq!(
            parse_arp_mac("? (192.168.11.1) at (incomplete) on en0 ifscope [ethernet]\n"),
            None
        );
    }
}
