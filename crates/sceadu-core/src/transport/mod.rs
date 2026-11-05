//! L1 transport: IPv6 TCP, relay, Tor plugin, connectivity policy.

use std::net::Ipv6Addr;

use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

use crate::PeerId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("Tor plugin unavailable: {0}")]
    TorUnavailable(String),
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    #[error("transport error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod tor;

pub use tor::TorTransport;

/// How a node chooses a network path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityPolicy {
    /// Try direct TCP, then future fallbacks.
    #[default]
    Auto,
    /// Use only direct TCP addresses.
    Direct,
    /// Require a relay path.
    Relay,
    /// Require Tor.
    Privacy,
}

impl ConnectivityPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Direct => "direct",
            Self::Relay => "relay",
            Self::Privacy => "privacy",
        }
    }
}

/// Transport names used in status and future manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Tcp,
    Relay,
    Tor,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Relay => "relay",
            Self::Tor => "tor",
        }
    }
}

/// Locally observed reachability classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Loopback,
    Private,
    LinkLocal,
    PublicUnverified,
    PublicVerified,
    Relay,
    Tor,
    Unknown,
}

impl Reachability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link_local",
            Self::PublicUnverified => "public_unverified",
            Self::PublicVerified => "public_verified",
            Self::Relay => "relay",
            Self::Tor => "tor",
            Self::Unknown => "unknown",
        }
    }
}

/// One endpoint known to the local address book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointRecord {
    pub addr: String,
    pub transport: TransportKind,
    pub reachability: Reachability,
    pub source: String,
    pub verified: bool,
}

/// Availability of a transport in this build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportStatus {
    pub transport: TransportKind,
    pub available: bool,
    pub note: String,
}

/// A snapshot of local connectivity facts for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub agent_id: String,
    pub peer_id: String,
    pub listen: Option<String>,
    pub policy: ConnectivityPolicy,
    pub reachability: Reachability,
    pub transports: Vec<TransportStatus>,
    pub endpoints: Vec<EndpointRecord>,
    pub relay_reservation: Option<String>,
}

pub(crate) fn ensure_operation_allowed(
    policy: ConnectivityPolicy,
    relay_enabled: bool,
    relay_server: bool,
    tor_enabled: bool,
) -> Result<()> {
    match policy {
        ConnectivityPolicy::Auto | ConnectivityPolicy::Direct => Ok(()),
        ConnectivityPolicy::Relay if relay_enabled || relay_server => Ok(()),
        ConnectivityPolicy::Relay => Err(Error::Unsupported(
            "relay policy requires --relay or --relay-server".to_owned(),
        )),
        ConnectivityPolicy::Privacy if tor_enabled => Ok(()),
        ConnectivityPolicy::Privacy => Err(Error::Unsupported(
            "privacy policy requires --tor-proxy (Tor plugin)".to_owned(),
        )),
    }
}

pub(crate) fn ensure_dial_allowed(
    policy: ConnectivityPolicy,
    relay_enabled: bool,
    relay_server: bool,
    tor_enabled: bool,
    addr: &Multiaddr,
) -> Result<()> {
    ensure_ipv6_peer_addr(addr)?;
    match policy {
        ConnectivityPolicy::Auto => Ok(()),
        ConnectivityPolicy::Direct => {
            if is_relayed(addr) || is_onion(addr) {
                Err(Error::Unsupported(
                    "direct policy only allows clearnet TCP addresses".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
        ConnectivityPolicy::Relay => {
            if !is_relayed(addr) {
                return Err(Error::Unsupported(
                    "relay policy requires a /p2p-circuit address".to_owned(),
                ));
            }
            ensure_operation_allowed(policy, relay_enabled, relay_server, tor_enabled)
        }
        ConnectivityPolicy::Privacy => {
            if is_onion(addr) && !tor_enabled {
                return Err(Error::Unsupported(
                    "onion address requires --tor-proxy".to_owned(),
                ));
            }
            if !is_onion(addr) {
                return Err(Error::Unsupported(
                    "privacy policy requires an /onion3/ address".to_owned(),
                ));
            }
            ensure_operation_allowed(policy, relay_enabled, relay_server, tor_enabled)
        }
    }
}

fn peer_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address
        .iter()
        .filter_map(|protocol| match protocol {
            Protocol::P2p(peer) => Some(peer),
            _ => None,
        })
        .last()
}

pub(crate) fn ensure_peer_id(address: Multiaddr, peer_id: PeerId) -> Multiaddr {
    if peer_from_multiaddr(&address) == Some(peer_id) {
        address
    } else {
        address.with(Protocol::P2p(peer_id))
    }
}

pub fn ensure_ipv6_listen_addr(addr: &Multiaddr) -> Result<()> {
    let mut has_ip6 = false;
    let mut has_tcp = false;
    for protocol in addr.iter() {
        match protocol {
            Protocol::Ip4(_) => {
                return Err(Error::Unsupported(
                    "IPv4 is not supported; listen on a concrete /ip6 address".to_owned(),
                ));
            }
            Protocol::Ip6(ip) => {
                if ip.is_unspecified() {
                    return Err(Error::Unsupported(
                        "listen address must be a concrete IPv6 address, not ::".to_owned(),
                    ));
                }
                if ip.is_multicast() || ip.to_ipv4_mapped().is_some() {
                    return Err(Error::Unsupported(
                        "listen address must be a unicast IPv6 address".to_owned(),
                    ));
                }
                if ip.is_unicast_link_local() {
                    return Err(Error::Unsupported(
                        "listen address must not be link-local".to_owned(),
                    ));
                }
                has_ip6 = true;
            }
            Protocol::Tcp(_) => has_tcp = true,
            _ => {
                return Err(Error::Unsupported(
                    "listen must be a /ip6/<address>/tcp/<port> multiaddr".to_owned(),
                ));
            }
        }
    }
    if !has_ip6 || !has_tcp {
        return Err(Error::Unsupported(
            "listen must be a concrete /ip6/<address>/tcp/<port> multiaddr".to_owned(),
        ));
    }
    Ok(())
}

pub fn ensure_ipv6_peer_addr(addr: &Multiaddr) -> Result<()> {
    let mut has_target = false;
    for protocol in addr.iter() {
        match protocol {
            Protocol::Ip4(_) => {
                return Err(Error::Unsupported(
                    "IPv4 is not supported; use an IPv6 or anonymous address".to_owned(),
                ));
            }
            Protocol::Ip6(ip) => {
                if ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_unicast_link_local()
                    || ip.to_ipv4_mapped().is_some()
                {
                    return Err(Error::Unsupported(
                        "peer address must be a global, ULA, or loopback IPv6 address".to_owned(),
                    ));
                }
                has_target = true;
            }
            Protocol::Dns(_) | Protocol::Dns4(_) => {
                return Err(Error::Unsupported(
                    "DNS4/DNS clearnet addresses are not supported; use /dns6".to_owned(),
                ));
            }
            Protocol::Dns6(_) => has_target = true,
            Protocol::Onion3(_) => has_target = true,
            _ => {}
        }
    }
    if !has_target {
        return Err(Error::Unsupported(
            "address must contain /ip6, /dns6, or /onion3".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn classify_multiaddr(addr: &Multiaddr) -> Reachability {
    for protocol in addr.iter() {
        match protocol {
            Protocol::Ip6(ip) => return classify_ipv6(ip),
            Protocol::Onion3(_) => return Reachability::Tor,
            _ => {}
        }
    }
    Reachability::Unknown
}

fn classify_ipv6(ip: Ipv6Addr) -> Reachability {
    if ip.is_loopback() {
        Reachability::Loopback
    } else if ip.is_unspecified() || ip.is_multicast() || ip.to_ipv4_mapped().is_some() {
        Reachability::Unknown
    } else if ip.is_unique_local() {
        Reachability::Private
    } else if ip.is_unicast_link_local() {
        Reachability::LinkLocal
    } else {
        let segments = ip.segments();
        let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
        let benchmarking = segments[0] == 0x2001 && segments[1] == 0x0002;
        let discard = segments[0] == 0x0100 && segments[1..4] == [0, 0, 0];
        if documentation || benchmarking || discard {
            Reachability::Unknown
        } else if segments[0] & 0xe000 == 0x2000 {
            Reachability::PublicUnverified
        } else {
            Reachability::Unknown
        }
    }
}

pub(crate) fn is_onion(addr: &Multiaddr) -> bool {
    addr.iter()
        .any(|protocol| matches!(protocol, Protocol::Onion3(_)))
}

pub(crate) fn is_relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|protocol| protocol == Protocol::P2pCircuit)
}

pub(crate) fn endpoint_record(addr: &Multiaddr, source: &str, verified: bool) -> EndpointRecord {
    let relayed = is_relayed(addr);
    let onion = is_onion(addr);
    EndpointRecord {
        addr: addr.to_string(),
        transport: if relayed {
            TransportKind::Relay
        } else if onion {
            TransportKind::Tor
        } else {
            TransportKind::Tcp
        },
        reachability: if relayed {
            Reachability::Relay
        } else if onion {
            Reachability::Tor
        } else {
            classify_multiaddr(addr)
        },
        source: source.to_owned(),
        verified,
    }
}
