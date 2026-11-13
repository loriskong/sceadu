use sceadu_core::{
    spawn, spawn_with_config, AgentConfig, Identity, Multiaddr, NodeConfig, SessionConfig,
};

#[tokio::test]
async fn node_publishes_and_refreshes_biographies_through_bootstrap() {
    let bootstrap_identity = Identity::generate();
    let member_identity = Identity::generate();

    let (bootstrap, bootstrap_addr) = spawn(
        &bootstrap_identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
    )
    .await
    .expect("bootstrap starts");
    let bootstrap_addr = bootstrap_addr.expect("bootstrap listen address");

    let (member, _) = spawn_with_config(
        &member_identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
        NodeConfig {
            session: SessionConfig::default(),
            agent: AgentConfig {
                description: "tennis member".to_owned(),
                tags: vec!["tennis".to_owned()],
                ..Default::default()
            },
            bootstrap: vec![bootstrap_addr],
            ..Default::default()
        },
    )
    .await
    .expect("member starts");

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let bootstrap_view = bootstrap.biographies().expect("bootstrap store");
    assert!(bootstrap_view
        .iter()
        .any(|biography| biography.agent_id == member_identity.agent_id()));

    let refreshed = member.refresh_biographies().await.expect("refresh");
    assert!(refreshed
        .iter()
        .any(|biography| biography.agent_id == bootstrap_identity.agent_id()));

    member.shutdown().await;
    bootstrap.shutdown().await;
}
