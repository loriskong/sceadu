use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

fn init(directory: &TempDir, name: &str, passphrase: &Path) -> (String, PathBuf) {
    let identity = directory.path().join(format!("{name}.json"));
    let output = Command::new(env!("CARGO_BIN_EXE_sceadu"))
        .args([
            "--path",
            identity.to_str().unwrap(),
            "--passphrase-file",
            passphrase.to_str().unwrap(),
            "init",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    (value["agent_id"].as_str().unwrap().to_owned(), identity)
}

fn start_daemon(
    directory: &TempDir,
    name: &str,
    identity: &Path,
    passphrase: &Path,
) -> (Child, String, PathBuf) {
    let socket = directory.path().join(format!("{name}.sock"));
    let inbox = directory.path().join(format!("{name}.sqlite"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_sceadu"))
        .args([
            "--path",
            identity.to_str().unwrap(),
            "--passphrase-file",
            passphrase.to_str().unwrap(),
            "--socket",
            socket.to_str().unwrap(),
            "daemon",
            "--listen",
            "/ip6/::1/tcp/0",
            "--connectivity",
            "direct",
            "--inbox",
            inbox.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let ready = lines.next().unwrap().unwrap();
    let value: Value = serde_json::from_str(&ready).unwrap();
    let address = value["listen"].as_str().unwrap().to_owned();
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    (child, address, socket)
}

fn client(socket: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sceadu"))
        .args(["--socket", socket.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn two_daemons_exchange_and_reply() {
    let directory = TempDir::new().unwrap();
    let passphrase = directory.path().join("passphrase.txt");
    fs::write(&passphrase, "correct horse battery staple\n").unwrap();
    let (a_id, a_identity) = init(&directory, "a", &passphrase);
    let (b_id, b_identity) = init(&directory, "b", &passphrase);

    let (mut b, b_address, b_socket) = start_daemon(&directory, "b", &b_identity, &passphrase);
    let (mut a, a_address, a_socket) = start_daemon(&directory, "a", &a_identity, &passphrase);

    let message = directory.path().join("message.json");
    fs::write(
        &message,
        serde_json::to_vec(&json!({
            "protocol": "sceadu.agent-message/0.1",
            "message_id": "msg-1",
            "context_id": "ctx-1",
            "from": a_id,
            "to": b_id,
            "role": "user",
            "parts": [{"text": "hello"}],
            "created_at": 1
        }))
        .unwrap(),
    )
    .unwrap();
    let sent = client(
        &a_socket,
        &[
            "send",
            "--to",
            &b_address,
            "--message",
            message.to_str().unwrap(),
        ],
    );
    assert!(
        sent.status.success(),
        "send failed: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    let inbox = client(&b_socket, &["inbox", "--limit", "10"]);
    assert!(inbox.status.success());
    let messages: Value = serde_json::from_slice(&inbox.stdout).unwrap();
    assert_eq!(messages.as_array().unwrap().len(), 1);
    let message_id = messages[0]["message_id"].as_str().unwrap();

    let replied = client(
        &b_socket,
        &["reply", message_id, "--text", "hi", "--to", &a_address],
    );
    assert!(
        replied.status.success(),
        "reply failed: {}",
        String::from_utf8_lossy(&replied.stderr)
    );
    let inbox = client(&a_socket, &["inbox", "--limit", "10"]);
    assert!(inbox.status.success());
    let messages: Value = serde_json::from_slice(&inbox.stdout).unwrap();
    assert_eq!(messages.as_array().unwrap().len(), 1);
    assert_eq!(messages[0]["from"], b_id);

    let _ = a.kill();
    let _ = b.kill();
    let _ = a.wait();
    let _ = b.wait();
}
