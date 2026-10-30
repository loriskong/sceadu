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
fn manifest_round_trips_through_verify() {
    let directory = TempDir::new().unwrap();
    let init = run_json(&directory, "correct horse battery staple", &["init"]);
    assert!(init.status.success());

    let manifest = run_json(
        &directory,
        "correct horse battery staple",
        &["manifest", "--capability", "task.echo"],
    );
    let manifest_json = parse_json(&manifest);
    let manifest_path = directory.path().join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest_json).unwrap(),
    )
    .unwrap();

    let mut command = Command::cargo_bin("sceadu").unwrap();
    let output = command
        .args(["verify", manifest_path.to_str().unwrap()])
        .output()
        .unwrap();
    let verified = parse_json(&output);

    assert_eq!(verified["valid"], true);
    assert_eq!(
        verified["agent_id"],
        manifest_json["body"]["agent_id"].clone()
    );
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
