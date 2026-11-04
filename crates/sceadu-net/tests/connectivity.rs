use sceadu_core::Identity;
use std::time::Duration;

use sceadu_net::{
    spawn, spawn_with_config, ConnectivityPolicy, Multiaddr, NodeConfig, Reachability,
    TransportKind,
};

fn listen_addr() -> Multiaddr {
    "/ip6/::1/tcp/0".parse().unwrap()
}

#[tokio::test]
async fn status_reports_tcp_and_planned_transports() {
    let identity = Identity::generate();
    let (node, _) = spawn_with_config(
        &identity,
        Some(listen_addr()),
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Auto,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let status = node.network_status().await.unwrap();
    assert_eq!(status.policy, ConnectivityPolicy::Auto);
    assert_eq!(status.reachability, Reachability::Loopback);

    let tcp = status
        .transports
        .iter()
        .find(|transport| transport.transport == TransportKind::Tcp)
        .unwrap();
    assert!(tcp.available);
    for kind in [TransportKind::Relay, TransportKind::Tor, TransportKind::I2P] {
        let transport = status
            .transports
            .iter()
            .find(|transport| transport.transport == kind)
            .unwrap();
        assert!(!transport.available);
    }

    node.shutdown().await;
}

#[tokio::test]
async fn privacy_policy_blocks_direct_dial() {
    let server_identity = Identity::generate();
    let client_identity = Identity::generate();
    let (server, server_addr) = spawn(&server_identity, Some(listen_addr())).await.unwrap();
    let (client, _) = spawn_with_config(
        &client_identity,
        None,
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            tor_proxy: Some("[::1]:9050".parse().unwrap()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(client.dial(server_addr.unwrap()).await.is_err());

    client.shutdown().await;
    server.shutdown().await;
}

#[tokio::test]
async fn privacy_policy_rejects_cleartext_listener() {
    let identity = Identity::generate();
    let result = spawn_with_config(
        &identity,
        Some(listen_addr()),
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            tor_proxy: Some("[::1]:9050".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn relay_circuit_connects_two_peers() {
    let relay_identity = Identity::generate();
    let server_identity = Identity::generate();
    let client_identity = Identity::generate();

    let (relay, relay_addr) = tokio::time::timeout(
        Duration::from_secs(10),
        spawn_with_config(
            &relay_identity,
            Some(listen_addr()),
            NodeConfig {
                relay_server: true,
                ..Default::default()
            },
        ),
    )
    .await
    .expect("relay spawn timed out")
    .unwrap();
    let relay_addr = relay_addr.unwrap();
    let (server, server_addr) = tokio::time::timeout(
        Duration::from_secs(10),
        spawn_with_config(
            &server_identity,
            None,
            NodeConfig {
                relay: Some(relay_addr),
                ..Default::default()
            },
        ),
    )
    .await
    .expect("relay client spawn timed out")
    .unwrap();
    let server_addr = server_addr.expect("server reserves a relay circuit");
    assert!(server_addr.to_string().contains("p2p-circuit"));
    let status = server.network_status().await.unwrap();
    assert!(status
        .transports
        .iter()
        .any(|transport| transport.transport == TransportKind::Relay && transport.available));

    let (client, _) = spawn(&client_identity, None).await.unwrap();
    let server_peer = tokio::time::timeout(Duration::from_secs(10), client.dial(server_addr))
        .await
        .expect("relay dial timed out")
        .unwrap();
    let manifest = tokio::time::timeout(
        Duration::from_secs(10),
        client.request_manifest(server_peer),
    )
    .await
    .expect("relay request timed out")
    .unwrap();
    assert_eq!(manifest.body.agent_id, server_identity.agent_id());

    client.shutdown().await;
    server.shutdown().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn privacy_status_reports_tor_available() {
    let identity = Identity::generate();
    let (node, _) = spawn_with_config(
        &identity,
        None,
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            tor_proxy: Some("[::1]:9050".parse().unwrap()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let status = node.network_status().await.unwrap();
    assert_eq!(status.policy, ConnectivityPolicy::Privacy);
    assert!(status
        .transports
        .iter()
        .any(|transport| transport.transport == TransportKind::Tor && transport.available));

    node.shutdown().await;
}

#[tokio::test]
async fn privacy_loopback_listener_requires_onion() {
    let identity = Identity::generate();
    let result = spawn_with_config(
        &identity,
        Some(listen_addr()),
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            tor_proxy: Some("[::1]:9050".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn privacy_manifest_advertises_only_the_onion_endpoint() {
    use data_encoding::BASE32_NOPAD;

    let identity = Identity::generate();
    let mut hash = [0u8; 35];
    hash[0] = 1;
    let onion: Multiaddr = format!(
        "/onion3/{}:4001",
        BASE32_NOPAD.encode(&hash).to_ascii_lowercase()
    )
    .parse()
    .unwrap();

    let (node, _) = spawn_with_config(
        &identity,
        Some(listen_addr()),
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            tor_proxy: Some("[::1]:9050".parse().unwrap()),
            onion: Some(onion.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let status = node.network_status().await.unwrap();
    let expected = format!("{}/p2p/{}", onion, node.peer_id());
    assert_eq!(status.listen.as_deref(), Some(expected.as_str()));
    assert_eq!(status.reachability, Reachability::Tor);

    node.shutdown().await;
}

async fn handle_socks5(mut socket: tokio::net::TcpStream, target: std::net::SocketAddr) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut greeting = [0u8; 2];
    if socket.read_exact(&mut greeting).await.is_err() {
        return;
    }
    let mut methods = vec![0u8; greeting[1] as usize];
    if socket.read_exact(&mut methods).await.is_err() {
        return;
    }
    if socket.write_all(&[5, 0]).await.is_err() {
        return;
    }

    let mut request = [0u8; 4];
    if socket.read_exact(&mut request).await.is_err() {
        return;
    }
    let address_len = match request[3] {
        1 => 4,
        3 => {
            let mut len = [0u8; 1];
            if socket.read_exact(&mut len).await.is_err() {
                return;
            }
            len[0] as usize
        }
        4 => 16,
        _ => return,
    };
    let mut address = vec![0u8; address_len];
    if socket.read_exact(&mut address).await.is_err() {
        return;
    }
    let mut port = [0u8; 2];
    if socket.read_exact(&mut port).await.is_err() {
        return;
    }

    let Ok(mut upstream) = tokio::net::TcpStream::connect(target).await else {
        let _ = socket.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        return;
    };
    if socket
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
        .await
        .is_err()
    {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
}

#[tokio::test]
async fn privacy_tor_transport_dials_through_socks() {
    use data_encoding::BASE32_NOPAD;
    use std::net::{Ipv6Addr, SocketAddr};

    let server_identity = Identity::generate();
    let client_identity = Identity::generate();
    let (server, server_addr) = spawn(&server_identity, Some(listen_addr())).await.unwrap();
    let server_addr = server_addr.unwrap();
    let server_peer = server.peer_id();

    let port = server_addr
        .iter()
        .find_map(|protocol| match protocol {
            libp2p::multiaddr::Protocol::Tcp(port) => Some(port),
            _ => None,
        })
        .unwrap();
    let target = SocketAddr::from((Ipv6Addr::LOCALHOST, port));

    let proxy_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let proxy_addr = match proxy_listener.local_addr().unwrap() {
        SocketAddr::V6(address) => address,
        _ => unreachable!("IPv6 proxy listener"),
    };
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = proxy_listener.accept().await else {
                break;
            };
            tokio::spawn(handle_socks5(stream, target));
        }
    });

    let mut hash = [0u8; 35];
    hash[0] = 1;
    let onion: Multiaddr = format!(
        "/onion3/{}:4001/p2p/{}",
        BASE32_NOPAD.encode(&hash).to_ascii_lowercase(),
        server_peer
    )
    .parse()
    .unwrap();

    let (client, _) = spawn_with_config(
        &client_identity,
        None,
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            tor_proxy: Some(proxy_addr),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let peer = client.dial(onion).await.unwrap();
    let manifest = client.request_manifest(peer).await.unwrap();
    assert_eq!(manifest.body.agent_id, server_identity.agent_id());

    client.shutdown().await;
    server.shutdown().await;
}

#[tokio::test]
async fn privacy_i2p_transport_dials_through_socks() {
    use std::net::{Ipv6Addr, SocketAddr};

    let server_identity = Identity::generate();
    let client_identity = Identity::generate();
    let (server, server_addr) = spawn(&server_identity, Some(listen_addr())).await.unwrap();
    let server_addr = server_addr.unwrap();
    let server_peer = server.peer_id();

    let port = server_addr
        .iter()
        .find_map(|protocol| match protocol {
            libp2p::multiaddr::Protocol::Tcp(port) => Some(port),
            _ => None,
        })
        .unwrap();
    let target = SocketAddr::from((Ipv6Addr::LOCALHOST, port));

    let proxy_listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let proxy_addr = match proxy_listener.local_addr().unwrap() {
        SocketAddr::V6(address) => address,
        _ => unreachable!("IPv6 proxy listener"),
    };
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = proxy_listener.accept().await else {
                break;
            };
            tokio::spawn(handle_socks5(stream, target));
        }
    });

    let i2p: Multiaddr = format!("/dns4/example.b32.i2p/tcp/80/p2p/{server_peer}")
        .parse()
        .unwrap();

    let (client, _) = spawn_with_config(
        &client_identity,
        None,
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            i2p_proxy: Some(proxy_addr),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let peer = client.dial(i2p).await.unwrap();
    let manifest = client.request_manifest(peer).await.unwrap();
    assert_eq!(manifest.body.agent_id, server_identity.agent_id());

    client.shutdown().await;
    server.shutdown().await;
}

#[tokio::test]
async fn privacy_status_reports_i2p_available() {
    let identity = Identity::generate();
    let (node, _) = spawn_with_config(
        &identity,
        None,
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            i2p_proxy: Some("[::1]:4447".parse().unwrap()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let status = node.network_status().await.unwrap();
    assert!(status
        .transports
        .iter()
        .any(|transport| transport.transport == TransportKind::I2P && transport.available));

    node.shutdown().await;
}

#[tokio::test]
async fn privacy_manifest_advertises_only_the_i2p_endpoint() {
    let identity = Identity::generate();
    let i2p: Multiaddr = "/dns4/example.b32.i2p/tcp/80".parse().unwrap();

    let (node, _) = spawn_with_config(
        &identity,
        Some(listen_addr()),
        NodeConfig {
            connectivity_policy: ConnectivityPolicy::Privacy,
            i2p_proxy: Some("[::1]:4447".parse().unwrap()),
            i2p_address: Some(i2p.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let status = node.network_status().await.unwrap();
    let expected = format!("{}/p2p/{}", i2p, node.peer_id());
    assert_eq!(status.listen.as_deref(), Some(expected.as_str()));
    assert_eq!(status.reachability, Reachability::I2P);

    node.shutdown().await;
}
