use std::time::Duration;

use sceadu_core::Identity;
use sceadu_net::{spawn, Multiaddr, TaskEventKind};
use serde_json::json;

fn listen_addr() -> Multiaddr {
    "/ip6/::1/tcp/0".parse().unwrap()
}

#[tokio::test]
async fn streams_echo_task_events() {
    let server_identity = Identity::generate();
    let client_identity = Identity::generate();

    let (server, server_addr) = spawn(&server_identity, Some(listen_addr())).await.unwrap();
    let (client, _) = spawn(&client_identity, None).await.unwrap();

    let peer = client.dial(server_addr.unwrap()).await.unwrap();
    client.request_manifest(peer).await.unwrap();

    let mut handle = client
        .ask(
            peer,
            "task.echo",
            json!({"text": "hi"}),
            Duration::from_secs(10),
        )
        .await
        .unwrap();

    let mut events = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(5), handle.next()).await {
        events.push(event);
        if matches!(events.last().unwrap().event, TaskEventKind::Done) {
            break;
        }
    }

    assert!(events
        .iter()
        .any(|event| matches!(event.event, TaskEventKind::Started { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.event, TaskEventKind::Progress { .. })));
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            TaskEventKind::Result { output } if output == &json!({"echo": {"text": "hi"}})
        )
    }));
    assert!(matches!(
        events.last().expect("done event").event,
        TaskEventKind::Done
    ));

    client.shutdown().await;
    server.shutdown().await;
}

#[tokio::test]
async fn rejects_unknown_capability() {
    let server_identity = Identity::generate();
    let client_identity = Identity::generate();

    let (server, server_addr) = spawn(&server_identity, Some(listen_addr())).await.unwrap();
    let (client, _) = spawn(&client_identity, None).await.unwrap();

    let peer = client.dial(server_addr.unwrap()).await.unwrap();
    client.request_manifest(peer).await.unwrap();

    let result = client
        .ask(peer, "task.missing", json!({}), Duration::from_secs(10))
        .await;
    assert!(result.is_err());

    client.shutdown().await;
    server.shutdown().await;
}
