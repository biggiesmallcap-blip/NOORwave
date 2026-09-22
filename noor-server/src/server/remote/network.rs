use super::RemoteAddress;
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceKind {
    Physical,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub id: String,
    pub label: String,
    pub kind: InterfaceKind,
    pub active: bool,
    pub multicast: bool,
    pub ipv4: Vec<Ipv4Addr>,
}

pub fn enumerate_interfaces() -> Vec<InterfaceSnapshot> {
    netdev::get_interfaces()
        .into_iter()
        .map(|interface| {
            let label = interface
                .friendly_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&interface.name)
                .to_string();
            InterfaceSnapshot {
                id: interface.name.clone(),
                label,
                kind: if interface.is_physical() {
                    InterfaceKind::Physical
                } else {
                    InterfaceKind::Other
                },
                active: interface.is_up() && !interface.is_loopback(),
                multicast: interface.is_multicast(),
                ipv4: interface
                    .ipv4
                    .iter()
                    .map(|network| network.addr())
                    .collect(),
            }
        })
        .collect()
}

pub fn rank_addresses(
    listener: SocketAddr,
    interfaces: &[InterfaceSnapshot],
) -> Vec<RemoteAddress> {
    let listener_ip = match listener.ip() {
        IpAddr::V4(ip) => ip,
        IpAddr::V6(_) => return Vec::new(),
    };
    let mut candidates = Vec::new();
    for interface in interfaces.iter().filter(|interface| interface.active) {
        for ip in
            interface.ipv4.iter().copied().filter(|ip| {
                usable_ipv4(*ip) && (listener_ip.is_unspecified() || listener_ip == *ip)
            })
        {
            candidates.push((
                interface.kind,
                interface.label.clone(),
                ip,
                RemoteAddress {
                    id: format!("{}:{ip}", interface.id),
                    label: interface.label.clone(),
                    url: format!("http://{ip}:{}/remote", listener.port()),
                    kind: match interface.kind {
                        InterfaceKind::Physical => "lan",
                        InterfaceKind::Other => "other",
                    }
                    .into(),
                    recommended: false,
                },
            ));
        }
    }
    candidates.sort_by(|left, right| {
        kind_rank(left.0)
            .cmp(&kind_rank(right.0))
            .then_with(|| left.1.to_lowercase().cmp(&right.1.to_lowercase()))
            .then_with(|| left.2.octets().cmp(&right.2.octets()))
    });
    let mut seen = HashSet::new();
    let mut addresses: Vec<_> = candidates
        .into_iter()
        .filter_map(|(_, _, ip, address)| seen.insert(ip).then_some(address))
        .collect();
    if let Some(recommended) = addresses.iter_mut().find(|address| address.kind == "lan") {
        recommended.recommended = true;
    }
    addresses
}

fn usable_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() && !ip.is_link_local()
}

fn kind_rank(kind: InterfaceKind) -> u8 {
    match kind {
        InterfaceKind::Physical => 0,
        InterfaceKind::Other => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(
        id: &str,
        label: &str,
        kind: InterfaceKind,
        addresses: &[&str],
    ) -> InterfaceSnapshot {
        InterfaceSnapshot {
            id: id.into(),
            label: label.into(),
            kind,
            active: true,
            multicast: true,
            ipv4: addresses.iter().map(|ip| ip.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn wildcard_listener_offers_only_usable_ipv4_and_ranks_physical_lan_first() {
        let interfaces = vec![
            interface("vpn", "WireGuard", InterfaceKind::Other, &["10.8.0.2"]),
            interface(
                "wifi",
                "Wi-Fi",
                InterfaceKind::Physical,
                &["192.168.1.24", "169.254.2.3", "224.0.0.1", "0.0.0.0"],
            ),
            interface(
                "ethernet",
                "Ethernet",
                InterfaceKind::Physical,
                &["10.0.0.9", "127.0.0.1"],
            ),
        ];

        let ranked = rank_addresses("0.0.0.0:43123".parse::<SocketAddr>().unwrap(), &interfaces);

        assert_eq!(
            ranked
                .iter()
                .map(|address| (
                    address.label.as_str(),
                    address.url.as_str(),
                    address.kind.as_str(),
                    address.recommended
                ))
                .collect::<Vec<_>>(),
            vec![
                ("Ethernet", "http://10.0.0.9:43123/remote", "lan", true),
                ("Wi-Fi", "http://192.168.1.24:43123/remote", "lan", false),
                ("WireGuard", "http://10.8.0.2:43123/remote", "other", false),
            ]
        );
        assert!(
            ranked
                .iter()
                .all(|address| !address.url.contains("0.0.0.0"))
        );
    }

    #[test]
    fn listener_compatibility_requires_an_exact_ipv4_and_never_recommends_only_vpn() {
        let interfaces = vec![
            interface("vpn", "VPN tunnel", InterfaceKind::Other, &["10.8.0.2"]),
            interface("wifi", "Wi-Fi", InterfaceKind::Physical, &["192.168.1.24"]),
        ];

        let vpn_only = rank_addresses("10.8.0.2:17600".parse().unwrap(), &interfaces);
        assert_eq!(vpn_only.len(), 1);
        assert_eq!(vpn_only[0].kind, "other");
        assert!(!vpn_only[0].recommended);
        assert!(rank_addresses("[::]:17600".parse().unwrap(), &interfaces).is_empty());
        assert!(rank_addresses("192.168.1.99:17600".parse().unwrap(), &interfaces).is_empty());
    }

    #[test]
    fn duplicate_address_prefers_physical_lan_regardless_of_enumeration_order() {
        let interfaces = vec![
            interface("vpn", "A VPN", InterfaceKind::Other, &["192.168.1.24"]),
            interface("wifi", "Wi-Fi", InterfaceKind::Physical, &["192.168.1.24"]),
        ];

        let ranked = rank_addresses("0.0.0.0:17600".parse().unwrap(), &interfaces);

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].label, "Wi-Fi");
        assert_eq!(ranked[0].kind, "lan");
        assert!(ranked[0].recommended);
    }
}
