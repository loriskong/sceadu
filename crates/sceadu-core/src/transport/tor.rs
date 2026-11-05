//! Minimal Tor outbound transport over a SOCKS5 proxy.
//!
//! Inbound onion service mapping is handled by an external Tor daemon; this
//! transport only dials `.onion` multiaddresses.

use std::future::Ready;
use std::io;
use std::net::{SocketAddr, SocketAddrV6};
use std::pin::Pin;
use std::task::{Context, Poll};

use data_encoding::BASE32_NOPAD;
use futures::future::BoxFuture;
use libp2p::core::transport::{DialOpts, ListenerId, Transport, TransportError, TransportEvent};
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

type TorDial = BoxFuture<'static, Result<Compat<TcpStream>, io::Error>>;

/// Dial-only transport for `/onion3/...` multiaddresses through a SOCKS5 proxy.
#[derive(Clone)]
pub struct TorTransport {
    proxy: Option<SocketAddrV6>,
}

impl TorTransport {
    pub fn new(proxy: Option<SocketAddrV6>) -> Self {
        Self { proxy }
    }
}

impl Transport for TorTransport {
    type Output = Compat<TcpStream>;
    type Error = io::Error;
    type ListenerUpgrade = Ready<Result<Self::Output, Self::Error>>;
    type Dial = TorDial;

    fn listen_on(
        &mut self,
        _id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        Err(TransportError::MultiaddrNotSupported(addr))
    }

    fn remove_listener(&mut self, _id: ListenerId) -> bool {
        false
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        _opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        let (host, port) = onion_target(&addr)?;
        let proxy = self.proxy.ok_or_else(|| {
            TransportError::Other(io::Error::new(
                io::ErrorKind::Unsupported,
                "Tor proxy is not configured",
            ))
        })?;

        Ok(Box::pin(async move {
            let stream = Socks5Stream::connect(SocketAddr::V6(proxy), (host.as_str(), port))
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;
            Ok(stream.into_inner().compat())
        }))
    }

    fn poll(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Poll::Pending
    }
}

fn onion_target(addr: &Multiaddr) -> Result<(String, u16), TransportError<io::Error>> {
    let (hash, port) = addr
        .iter()
        .find_map(|protocol| match protocol {
            Protocol::Onion3(onion) => Some((*onion.hash(), onion.port())),
            _ => None,
        })
        .ok_or_else(|| TransportError::MultiaddrNotSupported(addr.clone()))?;
    let host = BASE32_NOPAD.encode(&hash).to_ascii_lowercase();
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_encoding::BASE32_NOPAD;
    use libp2p::core::connection::Endpoint;
    use libp2p::core::transport::PortUse;

    #[test]
    fn parses_onion_address() {
        let mut hash = [0u8; 35];
        hash[0] = 1;
        let encoded = BASE32_NOPAD.encode(&hash).to_ascii_lowercase();
        let addr: Multiaddr = format!("/onion3/{encoded}:4001").parse().unwrap();

        let (host, port) = onion_target(&addr).unwrap();
        assert_eq!(host, encoded);
        assert_eq!(port, 4001);
    }

    #[test]
    fn rejects_non_onion_address() {
        let addr: Multiaddr = "/ip6/::1/tcp/4001".parse().unwrap();
        assert!(onion_target(&addr).is_err());
    }

    #[test]
    fn dial_without_proxy_fails() {
        let mut transport = TorTransport::new(None);
        let addr: Multiaddr = format!("/onion3/{}:4001", "a".repeat(56)).parse().unwrap();
        let opts = DialOpts {
            role: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        };
        assert!(transport.dial(addr, opts).is_err());
    }
}
