use sceadu_core::Identity;
use std::time::Duration;

use sceadu_net::{
    spawn, spawn_with_config, ConnectivityPolicy, Multiaddr, NodeConfig, Reachability,
    TransportKind,
};

fn listen_addr() -> Multiaddr {
    "/ip4/127.0.0.1/tcp/0".parse().unwrap()
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
async fn port_mapping_is_reported_as_requested() {
    let identity = Identity::generate();
    let (node, _) = spawn_with_config(
        &identity,
        Some(listen_addr()),
        NodeConfig {
            port_mapping: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let status = node.network_status().await.unwrap();
    assert!(status.port_mapping);
    assert!(status
        .transports
        .iter()
        .any(|transport| transport.transport == TransportKind::PortMapping && transport.available));

    node.shutdown().await;
}
