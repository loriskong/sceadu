use sceadu_core::Identity;
use sceadu_core::{
    spawn, spawn_with_config, AgentMessage, DiscoveryConfig, Multiaddr, NeighborPolicy, NodeConfig,
    Part, Role, TextPart,
};

fn message(from: &Identity, to: &Identity, text: &str) -> AgentMessage {
    AgentMessage::new(
        sceadu_core::agent::new_message_id(),
        sceadu_core::agent::new_context_id(),
        from.agent_id().to_string(),
        to.agent_id().to_string(),
        Role::User,
        vec![Part::Text(TextPart {
            text: text.to_owned(),
        })],
    )
}

fn listen_addr() -> Multiaddr {
    "/ip6/::1/tcp/0".parse().unwrap()
}

#[tokio::test]
async fn neighbor_query_discovers_a_third_node() {
    let a_identity = Identity::generate();
    let b_identity = Identity::generate();
    let c_identity = Identity::generate();

    let (a, a_addr) = spawn(&a_identity, Some(listen_addr())).await.unwrap();
    let a_addr = a_addr.unwrap();
    let (b, b_addr) = spawn(&b_identity, Some(listen_addr())).await.unwrap();
    let b_addr = b_addr.unwrap();
    let (c, c_addr) = spawn(&c_identity, Some(listen_addr())).await.unwrap();
    let c_addr = c_addr.unwrap();

    // B learns C's signed manifest; C learns B's.
    let c_peer = b.dial(c_addr).await.unwrap();
    b.request_manifest(c_peer).await.unwrap();

    // A learns B's signed manifest; B learns A's.
    let b_peer = a.dial(b_addr).await.unwrap();
    a.request_manifest(b_peer).await.unwrap();

    let records = a
        .neighbors(b_peer, Some("tennis".to_owned()), 8)
        .await
        .unwrap();
    let c_record = records
        .iter()
        .find(|record| record.manifest.body.agent_id == c_identity.agent_id())
        .expect("B introduces C");

    let c_endpoint: Multiaddr = c_record.manifest.body.endpoints[0].parse().unwrap();
    let c_peer = a.dial(c_endpoint).await.unwrap();
    let c_manifest = a.request_manifest(c_peer).await.unwrap();
    assert_eq!(c_manifest.body.agent_id, c_identity.agent_id());

    a.send_message(c_peer, message(&a_identity, &c_identity, "hello"))
        .await
        .unwrap();
    let inbox = c.inbox().list(10).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from, a_identity.agent_id().to_string());

    let merged = a
        .discover(&[b_peer, c_peer], Some("tennis".to_owned()), 8, 8)
        .await
        .unwrap();
    assert!(merged.len() >= 2);
    assert!(merged
        .iter()
        .any(|record| record.manifest.body.agent_id == c_identity.agent_id()));

    a.shutdown().await;
    b.shutdown().await;
    c.shutdown().await;
    let _ = a_addr;
}

#[tokio::test]
async fn stealth_policy_returns_no_neighbors() {
    let a_identity = Identity::generate();
    let b_identity = Identity::generate();

    let (a, _) = spawn(&a_identity, Some(listen_addr())).await.unwrap();
    let (b, b_addr) = spawn_with_config(
        &b_identity,
        Some(listen_addr()),
        NodeConfig {
            discovery: DiscoveryConfig {
                neighbor_policy: NeighborPolicy::Stealth,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let b_addr = b_addr.unwrap();

    let b_peer = a.dial(b_addr).await.unwrap();
    a.request_manifest(b_peer).await.unwrap();

    let records = a.neighbors(b_peer, None, 8).await.unwrap();
    assert!(records.is_empty());

    a.shutdown().await;
    b.shutdown().await;
}
