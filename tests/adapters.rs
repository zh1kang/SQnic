#![cfg(unix)]
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn run(db: &Path, repo: &Path, harness: &str, remove: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sqnic"));
    command
        .arg("--db")
        .arg(db)
        .arg("setup")
        .arg("--repo")
        .arg(repo)
        .arg("--harness")
        .arg(harness);
    if remove {
        command.arg("--remove");
    }
    command.output().expect("run sqnic setup")
}

fn init_repo(repo: &Path) {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=SQnic test",
            "-c",
            "user.email=sqnic-test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "fixture",
        ])
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn claude_install_is_idempotent_and_preserves_existing_hooks() {
    let root = tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let config_dir = repo.join(".claude");
    fs::create_dir(&config_dir).unwrap();
    let config = config_dir.join("settings.json");
    fs::write(
        &config,
        r#"{"permissions":{"defaultMode":"dontAsk"},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"custom-command"}]}]}}"#,
    )
    .unwrap();
    let db = root.path().join("context.sqlite3");

    let first = run(&db, &repo, "claude", false);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("custom-command"));
    assert!(installed.contains("sqnic-managed:v1:claude"));
    let second = run(&db, &repo, "claude", false);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(installed, fs::read_to_string(&config).unwrap());

    let removed = run(&db, &repo, "claude", true);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let remaining: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(remaining["permissions"]["defaultMode"], "dontAsk");
    assert_eq!(
        remaining["hooks"]["Stop"][0]["hooks"][0]["command"],
        "custom-command"
    );
}

#[test]
fn malformed_config_is_rejected_without_replacement() {
    let root = tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let config_dir = repo.join(".codex");
    fs::create_dir(&config_dir).unwrap();
    let config = config_dir.join("hooks.json");
    fs::write(&config, "[]").unwrap();
    let original = fs::read(&config).unwrap();
    let output = run(&root.path().join("db"), &repo, "codex", false);
    assert!(!output.status.success());
    assert_eq!(original, fs::read(&config).unwrap());
}

#[test]
fn changed_or_grouped_owned_claude_hook_is_rejected() {
    let root = tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let db = root.path().join("db");
    let first = run(&db, &repo, "claude", false);
    assert!(first.status.success());
    let config = repo.join(".claude/settings.json");
    let mut value: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    value["hooks"]["SessionStart"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type":"command","command":"unrelated"}));
    fs::write(&config, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let original = fs::read(&config).unwrap();
    let second = run(&db, &repo, "claude", false);
    assert!(!second.status.success());
    assert_eq!(original, fs::read(&config).unwrap());
}

#[test]
fn pi_refuses_to_replace_an_unrelated_extension() {
    let root = tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let extension_dir = repo.join(".pi/extensions");
    fs::create_dir_all(&extension_dir).unwrap();
    let extension = extension_dir.join("sqnic.ts");
    fs::write(&extension, "export default () => {};\n").unwrap();
    let original = fs::read(&extension).unwrap();
    let output = run(&root.path().join("db"), &repo, "pi", false);
    assert!(!output.status.success());
    assert_eq!(original, fs::read(&extension).unwrap());
}

#[test]
fn pi_refuses_to_overwrite_a_modified_owned_extension() {
    let root = tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let db = root.path().join("db");
    let first = run(&db, &repo, "pi", false);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let extension = repo.join(".pi/extensions/sqnic.ts");
    let mut modified = fs::read(&extension).unwrap();
    modified.extend_from_slice(b"\n// local edit\n");
    fs::write(&extension, &modified).unwrap();
    let second = run(&db, &repo, "pi", false);
    assert!(!second.status.success());
    assert_eq!(modified, fs::read(&extension).unwrap());
    let text = String::from_utf8(modified).unwrap();
    assert!(text.contains("getSessionId"));
    assert!(text.contains("getSessionFile"));
    assert!(text.contains("session_start"));
}

#[test]
fn generated_hook_executes_with_quoted_paths() {
    use std::io::Write;
    use std::process::Stdio;
    let root = tempdir().unwrap();
    let repo = root.path().join("repo 'quoted' space");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let repo = fs::canonicalize(repo).unwrap();
    let db = root.path().join("db 'quoted'.sqlite3");
    assert!(run(&db, &repo, "claude", false).status.success());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO auto_leases VALUES(?,'fixture',unixepoch()+3600)",
        [repo.to_str().unwrap()],
    )
    .unwrap();
    let config: Value =
        serde_json::from_slice(&fs::read(repo.join(".claude/settings.json")).unwrap()).unwrap();
    let command = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let mut child = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        serde_json::json!({"cwd":repo,"session_id":"quoted","hook_event_name":"SessionStart"})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response["hookSpecificOutput"]["additionalContext"].is_string());
}

#[test]
fn symlinked_config_is_never_replaced() {
    let root = tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    fs::create_dir(repo.join(".claude")).unwrap();
    let target = root.path().join("user-settings.json");
    fs::write(&target, "{}").unwrap();
    let config = repo.join(".claude/settings.json");
    std::os::unix::fs::symlink(&target, &config).unwrap();
    assert!(
        !run(&root.path().join("db"), &repo, "claude", false)
            .status
            .success()
    );
    assert!(
        fs::symlink_metadata(&config)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
}
