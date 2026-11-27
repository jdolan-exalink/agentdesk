use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// Parses a MAC address from `AA:BB:CC:DD:EE:FF`, `AA-BB-CC-DD-EE-FF`, or `AABBCCDDEEFF`.
pub fn parse_mac(input: &str) -> Option<[u8; 6]> {
    let hex_only: String = input
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if hex_only.len() != 12 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, slot) in mac.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex_only[i * 2..i * 2 + 2], 16).ok()?;
    }
    if mac == [0u8; 6] || mac == [0xFF; 6] {
        return None;
    }
    Some(mac)
}

pub fn format_mac(mac: [u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Builds a standard 102-byte AMD Wake-on-LAN Magic Packet (`6 x 0xFF` + `16 x MAC`).
pub fn build_wol_magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut pkt = [0u8; 102];
    pkt[..6].fill(0xFF);
    for chunk in pkt[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&mac);
    }
    pkt
}

/// Derives directed /24 subnet broadcast targets from reported LAN IPv4 addresses.
pub fn derive_broadcast_targets(lan_ips: &[String]) -> Vec<SocketAddr> {
    let mut addrs = BTreeSet::new();
    addrs.insert(SocketAddr::from((Ipv4Addr::BROADCAST, 9)));
    addrs.insert(SocketAddr::from((Ipv4Addr::BROADCAST, 7)));
    for ip_str in lan_ips {
        if let Ok(ip) = ip_str.trim().parse::<Ipv4Addr>() {
            if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() {
                let octets = ip.octets();
                let bcast = Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
                addrs.insert(SocketAddr::from((bcast, 9)));
                addrs.insert(SocketAddr::from((bcast, 7)));
            }
        }
    }
    addrs.into_iter().collect()
}

/// Sends Wake-on-LAN UDP magic packets to both global broadcast and subnet broadcasts.
pub fn send_wol_magic_packets(macs: &[String], lan_ips: &[String]) -> usize {
    let parsed_macs: Vec<[u8; 6]> = macs.iter().filter_map(|m| parse_mac(m)).collect();
    if parsed_macs.is_empty() {
        return 0;
    }
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return 0;
    };
    let _ = sock.set_broadcast(true);
    let targets = derive_broadcast_targets(lan_ips);
    let mut sent = 0usize;
    for mac in parsed_macs {
        let packet = build_wol_magic_packet(mac);
        for target in &targets {
            if sock.send_to(&packet, target).is_ok() {
                sent += 1;
            }
        }
    }
    sent
}

#[cfg(windows)]
pub fn enumerate_local_mac_and_ips() -> (Vec<String>, Vec<String>) {
    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    // IfOperStatusUp = 1, MIB_IF_TYPE_LOOPBACK = 24, IF_TYPE_TUNNEL = 131
    const IF_OPER_STATUS_UP: i32 = 1;
    const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
    const IF_TYPE_IEEE80211: u32 = 71;

    let mut macs = BTreeSet::new();
    let mut ips = BTreeSet::new();

    unsafe {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut buf_len: u32 = 16 * 1024;
        let mut buf = vec![0u8; buf_len as usize];
        let mut ret = GetAdaptersAddresses(
            AF_INET.0 as u32,
            flags,
            None,
            Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
            &mut buf_len,
        );
        if ret == ERROR_BUFFER_OVERFLOW.0 {
            buf.resize(buf_len as usize, 0);
            ret = GetAdaptersAddresses(
                AF_INET.0 as u32,
                flags,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut buf_len,
            );
        }
        if ret != NO_ERROR.0 {
            return (Vec::new(), Vec::new());
        }

        let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !cur.is_null() {
            let adapter = &*cur;
            let is_up = adapter.OperStatus.0 == IF_OPER_STATUS_UP;
            let is_phys_nic = adapter.IfType == IF_TYPE_ETHERNET_CSMACD
                || adapter.IfType == IF_TYPE_IEEE80211;
            if is_up && is_phys_nic && adapter.PhysicalAddressLength == 6 {
                let mut raw = [0u8; 6];
                raw.copy_from_slice(&adapter.PhysicalAddress[..6]);
                if raw != [0u8; 6] && raw != [0xFF; 6] {
                    macs.insert(format_mac(raw));
                }
                let mut ucur = adapter.FirstUnicastAddress;
                while !ucur.is_null() {
                    let uaddr = &*ucur;
                    let sa = uaddr.Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr));
                        if !addr.is_loopback() && !addr.is_link_local() && !addr.is_unspecified() {
                            ips.insert(addr.to_string());
                        }
                    }
                    ucur = uaddr.Next;
                }
            }
            cur = adapter.Next;
        }
    }

    (macs.into_iter().collect(), ips.into_iter().collect())
}

#[cfg(not(windows))]
pub fn enumerate_local_mac_and_ips() -> (Vec<String>, Vec<String>) {
    (Vec::new(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_various_mac_formats_and_builds_magic_packet() {
        let a = parse_mac("AA:BB:CC:DD:EE:FF").unwrap();
        let b = parse_mac("aa-bb-cc-dd-ee-ff").unwrap();
        let c = parse_mac("aabbccddeeff").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(format_mac(a), "AA:BB:CC:DD:EE:FF");
        assert!(parse_mac("00:00:00:00:00:00").is_none());
        assert!(parse_mac("FF:FF:FF:FF:FF:FF").is_none());

        let pkt = build_wol_magic_packet(a);
        assert_eq!(pkt.len(), 102);
        assert_eq!(&pkt[..6], &[0xFF; 6]);
        for rep in pkt[6..].chunks_exact(6) {
            assert_eq!(rep, &a);
        }
    }

    #[test]
    fn derives_subnet_broadcast_targets() {
        let targets = derive_broadcast_targets(&["192.168.10.42".into(), "127.0.0.1".into()]);
        assert!(targets.contains(&SocketAddr::from((Ipv4Addr::new(192, 168, 10, 255), 9))));
        assert!(targets.contains(&SocketAddr::from((Ipv4Addr::BROADCAST, 9))));
        assert!(!targets.contains(&SocketAddr::from((Ipv4Addr::new(127, 0, 0, 255), 9))));
    }
}
