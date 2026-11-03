//! Minimal I2P outbound transport over the local router's SOCKS5 proxy.
//!
//! Inbound I2P service tunnels are configured in the I2P router and mapped to a
//! local loopback listener. This transport only dials `.i2p` names.

use std::future::Ready;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::future::BoxFuture;
use libp2p::core::transport::{DialOpts, ListenerId, Transport, TransportError, TransportEvent};
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

type I2pDial = BoxFuture<'static, Result<Compat<TcpStream>, io::Error>>;

/// Dial-only transport for `/dns4/<name>.i2p/...` multiaddresses.
#[derive(Clone)]
pub struct I2pTransport {
    proxy: Option<SocketAddr>,
}

impl I2pTransport {
    pub fn new(proxy: Option<SocketAddr>) -> Self {
        Self { proxy }
    }
}

impl Transport for I2pTransport {
    type Output = Compat<TcpStream>;
    type Error = io::Error;
    type ListenerUpgrade = Ready<Result<Self::Output, Self::Error>>;
    type Dial = I2pDial;

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
        let (host, port) = i2p_target(&addr)?;
        let proxy = self.proxy.ok_or_else(|| {
            TransportError::Other(io::Error::new(
                io::ErrorKind::Unsupported,
                "I2P proxy is not configured",
            ))
        })?;

        Ok(Box::pin(async move {
            let stream = Socks5Stream::connect(proxy, (host.as_str(), port))
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

fn i2p_target(addr: &Multiaddr) -> Result<(String, u16), TransportError<io::Error>> {
    let host = addr
        .iter()
        .find_map(|protocol| match protocol {
            Protocol::Dns(host) | Protocol::Dns4(host) | Protocol::Dns6(host) => {
                Some(host.to_string())
            }
            _ => None,
        })
        .ok_or_else(|| TransportError::MultiaddrNotSupported(addr.clone()))?;
    if !host.ends_with(".i2p") {
        return Err(TransportError::MultiaddrNotSupported(addr.clone()));
    }
    let port = addr
        .iter()
        .find_map(|protocol| match protocol {
            Protocol::Tcp(port) => Some(port),
            _ => None,
        })
        .unwrap_or(80);
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::core::connection::Endpoint;
    use libp2p::core::transport::PortUse;

    #[test]
    fn parses_i2p_address() {
        let addr: Multiaddr = "/dns4/example.b32.i2p/tcp/80".parse().unwrap();
        let (host, port) = i2p_target(&addr).unwrap();
        assert_eq!(host, "example.b32.i2p");
        assert_eq!(port, 80);
    }

    #[test]
    fn rejects_non_i2p_address() {
        let addr: Multiaddr = "/dns4/example.com/tcp/80".parse().unwrap();
        assert!(i2p_target(&addr).is_err());
    }

    #[test]
    fn dial_without_proxy_fails() {
        let mut transport = I2pTransport::new(None);
        let addr: Multiaddr = "/dns4/example.b32.i2p/tcp/80".parse().unwrap();
        let opts = DialOpts {
            role: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        };
        assert!(transport.dial(addr, opts).is_err());
    }
}
