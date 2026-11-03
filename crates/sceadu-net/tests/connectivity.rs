use sceadu_core::Identity;
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
