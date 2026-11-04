use sceadu_core::Identity;
use sceadu_net::{spawn, Multiaddr};

#[tokio::test]
async fn two_nodes_exchange_manifests_and_echo() {
    let server_identity = Identity::generate();
    let client_identity = Identity::generate();

    let (server, server_addr) = spawn(
        &server_identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
    )
    .await
    .expect("server starts");
    let server_addr = server_addr.expect("server has a listen address");

    let (client, _) = spawn(&client_identity, None).await.expect("client starts");

    let peer = client.dial(server_addr).await.expect("dial succeeds");
    assert_eq!(peer, server.peer_id());

    let manifest = client
        .request_manifest(peer)
        .await
        .expect("manifest exchange succeeds");
    assert_eq!(&manifest.body.agent_id, server.agent_id());

    let echo = client.echo(peer, "hello").await.expect("echo succeeds");
    assert_eq!(echo, "hello");

    client.shutdown().await;
    server.shutdown().await;
}
