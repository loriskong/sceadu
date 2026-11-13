use sceadu_core::{spawn, AgentMessage, Identity, Multiaddr, Part, Role, TextPart};

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

#[tokio::test]
async fn two_nodes_exchange_biographys_and_messages() {
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

    let biography = client
        .request_biography(peer)
        .await
        .expect("biography exchange succeeds");
    assert_eq!(&biography.agent_id, server.agent_id());

    client
        .send_message(peer, message(&client_identity, &server_identity, "hello"))
        .await
        .expect("message delivery succeeds");
    let inbox = server.inbox().list(10).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from, client_identity.agent_id().to_string());

    client.shutdown().await;
    server.shutdown().await;
}
