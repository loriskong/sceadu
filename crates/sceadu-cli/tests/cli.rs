use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn write_passphrase(directory: &TempDir, name: &str, value: &str) -> String {
    let path = directory.path().join(name);
    fs::write(&path, format!("{value}\n")).unwrap();
    path.display().to_string()
}

fn run_json(directory: &TempDir, passphrase: &str, args: &[&str]) -> std::process::Output {
    let passphrase_file = write_passphrase(directory, "passphrase.txt", passphrase);
    let identity_path = directory.path().join("identity.json");
    let mut command = Command::cargo_bin("sceadu").unwrap();
    command.args([
        "--path",
        identity_path.to_str().unwrap(),
        "--passphrase-file",
        passphrase_file.as_str(),
    ]);
    command.args(args);
    command.output().unwrap()
}

fn parse_json(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn init_and_id_keep_the_same_agent_id() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    let init_json = parse_json(&init);
    let agent_id = init_json["agent_id"].as_str().unwrap().to_owned();

    let id = run_json(&directory, "correct horse battery staple", &["id"]);
    let id_json = parse_json(&id);

    assert_eq!(id_json["agent_id"], agent_id);
}

#[test]
fn biography_round_trips_through_verify() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(init.status.success());

    let biography = run_json(
        &directory,
        "correct horse battery staple",
        &["biography", "--tag", "tennis"],
    );
    let biography_json = parse_json(&biography);
    let biography_path = directory.path().join("biography.json");
    fs::write(
        &biography_path,
        serde_json::to_vec_pretty(&biography_json).unwrap(),
    )
    .unwrap();

    let mut command = Command::cargo_bin("sceadu").unwrap();
    let output = command
        .args(["verify", biography_path.to_str().unwrap()])
        .output()
        .unwrap();
    let verified = parse_json(&output);

    assert_eq!(verified["valid"], true);
    assert_eq!(verified["agent_id"], biography_json["agent_id"].clone());
}

#[test]
fn wrong_passphrase_fails() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(init.status.success());

    let passphrase_file = write_passphrase(&directory, "wrong.txt", "wrong passphrase");
    let identity_path = directory.path().join("identity.json");
    let mut command = Command::cargo_bin("sceadu").unwrap();
    let output = command
        .args([
            "--path",
            identity_path.to_str().unwrap(),
            "--passphrase-file",
            passphrase_file.as_str(),
            "id",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unlock"));
}

#[test]
fn init_refuses_to_overwrite() {
    let directory = TempDir::new().unwrap();
    let first = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(first.status.success());

    let second = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already exists"));
}

#[test]
fn skill_install_writes_and_protects_skill() {
    let directory = TempDir::new().unwrap();
    let skill_dir = directory.path().join("sceadu-skill");
    let path = skill_dir.join("SKILL.md");

    let mut install = Command::cargo_bin("sceadu").unwrap();
    let first = install
        .args(["skill", "install", "--dir", skill_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(first.status.success());
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("name: sceadu"));

    let mut install_again = Command::cargo_bin("sceadu").unwrap();
    let second = install_again
        .args(["skill", "install", "--dir", skill_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already exists"));

    let mut force = Command::cargo_bin("sceadu").unwrap();
    let third = force
        .args([
            "skill",
            "install",
            "--dir",
            skill_dir.to_str().unwrap(),
            "--force",
        ])
        .output()
        .unwrap();
    assert!(third.status.success());
}

#[test]
fn daemon_status_reports_tcp_transport() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(init.status.success());
    let agent_id = parse_json(&init)["agent_id"].as_str().unwrap().to_owned();
    let passphrase_file =
        write_passphrase(&directory, "passphrase.txt", "correct horse battery staple");
    let identity_path = directory.path().join("identity.json");
    let socket = directory.path().join("daemon.sock");
    let inbox = directory.path().join("inbox.sqlite");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sceadu"))
        .args([
            "--path",
            identity_path.to_str().unwrap(),
            "--passphrase-file",
            passphrase_file.as_str(),
            "--socket",
            socket.to_str().unwrap(),
            "daemon",
            "--listen",
            "/ip6/::1/tcp/0",
            "--connectivity",
            "direct",
            "--allow-agent",
            &agent_id,
            "--inbox",
            inbox.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(socket.exists(), "daemon did not create its socket");
    let status = run_json_with_socket(&directory, &socket, &["status"]);
    let status_json = parse_json(&status);
    assert_eq!(status_json["policy"], "direct");
    assert!(status_json["transports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|transport| transport["transport"] == "tcp" && transport["available"] == true));
    let _ = child.kill();
    let _ = child.wait();
}

fn run_json_with_socket(
    directory: &TempDir,
    socket: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    let mut command = Command::cargo_bin("sceadu").unwrap();
    command.args(["--socket", socket.to_str().unwrap()]);
    command.args(args);
    let _ = directory;
    command.output().unwrap()
}

#[test]
fn daemon_rejects_ipv4_and_unspecified_listen() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(init.status.success());
    let agent_id = parse_json(&init)["agent_id"].as_str().unwrap().to_owned();
    let passphrase_file =
        write_passphrase(&directory, "passphrase.txt", "correct horse battery staple");
    let identity_path = directory.path().join("identity.json");
    for (index, listen) in ["/ip4/127.0.0.1/tcp/0", "/ip6/::/tcp/0"].iter().enumerate() {
        let socket = directory.path().join(format!("bad-{index}.sock"));
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sceadu"))
            .args([
                "--path",
                identity_path.to_str().unwrap(),
                "--passphrase-file",
                passphrase_file.as_str(),
                "--socket",
                socket.to_str().unwrap(),
                "daemon",
                "--listen",
                listen,
                "--allow-agent",
                &agent_id,
                "--inbox",
                directory.path().join("bad.sqlite").to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {listen}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("IPv4") || stderr.contains("concrete IPv6"),
            "unexpected error for {listen}: {stderr}"
        );
    }
}

#[test]
fn biography_rejects_ipv4_endpoint_and_accepts_ipv6() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(init.status.success());

    let passphrase_file =
        write_passphrase(&directory, "passphrase.txt", "correct horse battery staple");
    let identity_path = directory.path().join("identity.json");
    let mut bad = Command::cargo_bin("sceadu").unwrap();
    let bad_output = bad
        .args([
            "--path",
            identity_path.to_str().unwrap(),
            "--passphrase-file",
            passphrase_file.as_str(),
            "biography",
            "--endpoint",
            "/ip4/127.0.0.1/tcp/4001",
        ])
        .output()
        .unwrap();
    assert!(!bad_output.status.success());
    assert!(String::from_utf8_lossy(&bad_output.stderr).contains("IPv4"));

    let mut good = Command::cargo_bin("sceadu").unwrap();
    let good_output = good
        .args([
            "--path",
            identity_path.to_str().unwrap(),
            "--passphrase-file",
            passphrase_file.as_str(),
            "biography",
            "--endpoint",
            "/ip6/::1/tcp/4001",
        ])
        .output()
        .unwrap();
    let biography = parse_json(&good_output);
    assert_eq!(biography["endpoints"][0], "/ip6/::1/tcp/4001");
}

#[test]
fn mcp_initializes_and_lists_tools() {
    use std::io::{BufRead, BufReader, Write};
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sceadu"))
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
        )
        .unwrap();
        stdin.flush().unwrap();
    }
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let initialize: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(initialize["result"]["serverInfo"]["name"], "sceadu-mcp");
    let tools: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"sceadu_send"));
    assert!(names.contains(&"sceadu_send_text"));
    assert!(names.contains(&"sceadu_inbox"));
    let _ = child.kill();
    let _ = child.wait();
}
