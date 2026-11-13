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

#[tokio::test]
async fn biography_update_persists_and_republishes() {
    let directory = tempfile::TempDir::new().unwrap();
    let identity = Identity::generate();
    let store = directory.path().join("biographies.sqlite");
    let config = || NodeConfig {
        biography_store_path: Some(store.clone()),
        agent: AgentConfig {
            description: "old description".to_owned(),
            tags: vec!["tennis".to_owned()],
            ..Default::default()
        },
        ..Default::default()
    };

    let (node, _) = spawn_with_config(
        &identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
        config(),
    )
    .await
    .unwrap();

    let updated = node
        .update_biography(
            Some("new description".to_owned()),
            Some(vec!["tennis".to_owned(), "xuhui".to_owned()]),
        )
        .await
        .unwrap();
    assert_eq!(updated.description, "new description");
    assert!(updated.verify().is_ok());
    node.shutdown().await;

    let (node, _) = spawn_with_config(
        &identity,
        Some("/ip6/::1/tcp/0".parse::<Multiaddr>().unwrap()),
        config(),
    )
    .await
    .unwrap();
    let self_biography = node
        .biography(&identity.agent_id())
        .unwrap()
        .expect("self biography");
    assert_eq!(self_biography.description, "new description");
    assert!(self_biography.tags.contains(&"xuhui".to_owned()));
    node.shutdown().await;
}
