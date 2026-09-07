use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use tempfile::TempDir;
struct Fixture {
    dir: TempDir,
}
impl Fixture {
    fn new() -> Self {
        let f = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        f.git(&["init", "-q"]);
        f.run(&["unpause", "--repo", f.repo()]);
        let db = rusqlite::Connection::open(f.dir.path().join("db.sqlite")).unwrap();
        db.execute("INSERT INTO auto_leases(repo,token,expires) VALUES(?,'test-controlled',unixepoch()+3600)",[std::fs::canonicalize(f.dir.path()).unwrap().to_str().unwrap()]).unwrap();
        f
    }
    fn repo(&self) -> &str {
        self.dir.path().to_str().unwrap()
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_sqnic"));
        c.arg("--db").arg(self.dir.path().join("db.sqlite"));
        c
    }
    fn run(&self, args: &[&str]) -> Value {
        let o = self.command().args(args).output().unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn git(&self, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(self.repo())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fn hook(&self, harness: &str, session: &str, path: Option<&Path>, event: &str) -> Value {
        self.payload(harness,json!({"session_id":session,"cwd":self.repo(),"transcript_path":path,"hook_event_name":event}))
    }
    fn payload(&self, harness: &str, payload: Value) -> Value {
        let mut c = self
            .command()
            .args(["hook", "--repo", self.repo(), "--harness", harness])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(c.stdin.take().unwrap(), "{payload}").unwrap();
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success());
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn transcript(&self, id: &str, text: &str) -> std::path::PathBuf {
        let p = self.dir.path().join(format!("{id}.jsonl"));
        fs::write(&p,format!("{}\n",json!({"type":"user","sessionId":id,"cwd":self.repo(),"message":{"role":"user","content":text}}))).unwrap();
        p
    }
    fn status(&self) -> Value {
        self.run(&["auto-status", "--repo", self.repo()])
    }
}
fn context(v: &Value) -> Value {
    serde_json::from_str(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .split_once('\n')
            .unwrap()
            .1,
    )
    .unwrap()
}
#[test]
fn another_harness_restores_automatically_and_does_not_reinject_each_prompt() {
    let f = Fixture::new();
    let path = f.transcript("claude-1", "retain switch-marker-871 and use seven retries");
    let first = context(&f.hook("claude", "claude-1", Some(&path), "SessionStart"));
    assert_eq!(first["status"], "restored");
    let second = context(&f.hook("codex", "codex-1", None, "SessionStart"));
    assert_eq!(second["task"], first["task"]);
    assert!(second.to_string().contains("switch-marker-871"));
    assert!(
        f.hook("codex", "codex-1", None, "UserPromptSubmit")
            .get("hookSpecificOutput")
            .is_none()
    );
    let repeat = f.run(&["record", "--repo", f.repo(), "--once"]);
    assert_eq!(repeat["added"], 0);
    assert!(
        f.run(&["restore", "--repo", f.repo(), "--max-bytes", "2048"])
            .to_string()
            .len()
            <= 2048
    );
}
#[test]
fn ambiguous_tasks_require_selection_and_binding_cannot_be_retargeted() {
    let f = Fixture::new();
    for task in ["a", "b"] {
        f.run(&["create", task, "--repo", f.repo()]);
    }
    let initial = context(&f.hook("codex", "c", None, "SessionStart"));
    assert_eq!(initial["status"], "selection_required");
    let chosen = f.run(&[
        "restore",
        "--repo",
        f.repo(),
        "--harness",
        "codex",
        "--session",
        "c",
        "--task",
        "b",
    ]);
    assert_eq!(chosen["task"], "b");
    let out = f
        .command()
        .args([
            "restore",
            "--repo",
            f.repo(),
            "--harness",
            "codex",
            "--session",
            "c",
            "--task",
            "a",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let new = context(&f.hook("claude", "d", None, "SessionStart"));
    assert_eq!(new["status"], "selection_required");
}
#[test]
fn pause_excludes_current_sessions_without_later_private_backfill() {
    let f = Fixture::new();
    let path = f.transcript("s", "public marker");
    let initial = context(&f.hook("claude", "s", Some(&path), "SessionStart"));
    let task = initial["task"].as_str().unwrap();
    f.run(&["pause", "--repo", f.repo()]);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"text\":\"private marker\"}\n")
        .unwrap();
    f.run(&["unpause", "--repo", f.repo()]);
    assert!(
        f.hook("claude", "s", Some(&path), "UserPromptSubmit")
            .get("hookSpecificOutput")
            .is_none()
    );
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert!(
        f.run(&["search", task, "private"])["matches"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let fresh = context(&f.hook("codex", "new-session", None, "SessionStart"));
    assert_ne!(fresh["task"], initial["task"]);
}
#[test]
fn delayed_partial_and_rewritten_transcripts_have_visible_recovery_states() {
    let f = Fixture::new();
    let path = f.dir.path().join("s.jsonl");
    f.hook("claude", "s", Some(&path), "SessionStart");
    assert!(!f.status()["files"][0]["error"].is_null());
    f.transcript("s", "available after delay");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"text\":\"partial")
        .unwrap();
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert!(f.status()["files"][0]["pending_bytes"].as_i64().unwrap() > 0);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b" record\"}\n")
        .unwrap();
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert_eq!(f.status()["files"][0]["pending_bytes"], 0);
    f.transcript("s", "rewritten original");
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert!(!f.status()["files"][0]["error"].is_null());
}
#[test]
fn unrelated_project_transcripts_are_rejected_and_branch_changes_are_explicit() {
    let f = Fixture::new();
    let other = Fixture::new();
    let foreign = other.transcript("s", "foreign secret");
    let path = f.dir.path().join("s.jsonl");
    fs::copy(foreign, &path).unwrap();
    f.hook("claude", "s", Some(&path), "SessionStart");
    assert!(
        f.status()["files"][0]["error"]
            .as_str()
            .unwrap()
            .contains("another worktree")
    );
    f.git(&["checkout", "-qb", "other"]);
    let result = f.hook("claude", "s", Some(&path), "UserPromptSubmit");
    assert!(
        result["systemMessage"]
            .as_str()
            .unwrap()
            .contains("branch changed")
    );
    assert_eq!(f.status()["sessions"][0]["excluded"], true);
}

#[test]
fn restore_reports_required_state_that_does_not_fit() {
    let f = Fixture::new();
    f.run(&["create", "large", "--repo", f.repo()]);
    f.run(&[
        "update",
        "large",
        "--kind",
        "constraint",
        "--text",
        &"critical ".repeat(500),
    ]);
    let v = f.run(&["restore", "--repo", f.repo(), "--max-bytes", "2048"]);
    assert_eq!(v["status"], "required_state_omitted");
    assert!(v.to_string().len() <= 2048);
}

#[test]
fn concurrent_hooks_keep_one_task_and_recorder_lease_can_be_reclaimed() {
    let f = Fixture::new();
    let mut children = Vec::new();
    for id in ["a", "b", "c", "d"] {
        let payload = json!({"cwd":f.repo(),"session_id":id,"hook_event_name":"SessionStart"});
        let mut child = f
            .command()
            .args(["hook", "--repo", f.repo(), "--harness", "codex"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{payload}").unwrap();
        children.push(child);
    }
    let mut tasks = std::collections::HashSet::new();
    for child in children {
        let output = child.wait_with_output().unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        tasks.insert(context(&value)["task"].as_str().unwrap().to_owned());
    }
    assert_eq!(tasks.len(), 1);
    let db = rusqlite::Connection::open(f.dir.path().join("db.sqlite")).unwrap();
    db.execute("UPDATE auto_leases SET expires=0", []).unwrap();
    let mut worker = f
        .command()
        .args(["record", "--repo", f.repo()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..50 {
        let token: String = db
            .query_row("SELECT token FROM auto_leases", [], |r| r.get(0))
            .unwrap();
        if token != "test-controlled" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let token: String = db
        .query_row("SELECT token FROM auto_leases", [], |r| r.get(0))
        .unwrap();
    assert_ne!(token, "test-controlled");
    worker.kill().unwrap();
    worker.wait().unwrap();
    db.execute("UPDATE auto_leases SET expires=0", []).unwrap();
    let mut replacement = f
        .command()
        .args(["record", "--repo", f.repo()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..50 {
        let new: String = db
            .query_row("SELECT token FROM auto_leases", [], |r| r.get(0))
            .unwrap();
        if new != token {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let new: String = db
        .query_row("SELECT token FROM auto_leases", [], |r| r.get(0))
        .unwrap();
    assert_ne!(new, token);
    replacement.kill().unwrap();
    replacement.wait().unwrap();
}

#[test]
fn sessions_created_during_pause_never_backfill_after_unpause() {
    let f = Fixture::new();
    f.run(&["pause", "--repo", f.repo()]);
    let path = f.transcript("private-session", "never backfill this private text");
    f.hook("claude", "private-session", Some(&path), "SessionStart");
    f.run(&["unpause", "--repo", f.repo()]);
    assert!(
        f.hook("claude", "private-session", Some(&path), "UserPromptSubmit")
            .get("hookSpecificOutput")
            .is_none()
    );
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert_eq!(f.status()["sessions"][0]["excluded"], true);
    assert!(f.status()["files"].as_array().unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn adapter_removal_stops_its_capture_without_disabling_other_installed_harnesses() {
    let f = Fixture::new();
    for harness in ["claude", "codex"] {
        f.run(&["setup", "--repo", f.repo(), "--harness", harness]);
    }
    let path = f.transcript("s", "before uninstall");
    f.hook("claude", "s", Some(&path), "SessionStart");
    f.run(&[
        "setup",
        "--repo",
        f.repo(),
        "--harness",
        "claude",
        "--remove",
    ]);
    assert_eq!(f.status()["enabled"], true);
    assert_eq!(f.status()["sessions"][0]["excluded"], true);
    assert!(
        f.hook("claude", "new", None, "SessionStart")
            .get("hookSpecificOutput")
            .is_none()
    );
    f.run(&[
        "setup",
        "--repo",
        f.repo(),
        "--harness",
        "codex",
        "--remove",
    ]);
    assert_eq!(f.status()["enabled"], false);
}

#[test]
fn external_matching_transcript_requires_explicit_import() {
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    let original = f.transcript("external", "private external record");
    let path = external.path().join("external.jsonl");
    fs::copy(original, &path).unwrap();
    f.hook("claude", "external", Some(&path), "SessionStart");
    assert!(
        f.status()["files"][0]["error"]
            .as_str()
            .unwrap()
            .contains("approved harness history roots")
    );
}

#[test]
fn commit_without_new_chat_is_detected_and_unchanged_git_is_not_checkpointed() {
    let f = Fixture::new();
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.hook("claude", "git", None, "SessionStart");
    f.run(&["record", "--repo", f.repo(), "--once"]);
    let conn = rusqlite::Connection::open(f.dir.path().join("db.sqlite")).unwrap();
    let count = || {
        conn.query_row("SELECT count(*) FROM checkpoints", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let before = count();
    // Ignore fixture database and lock files so unchanged Git state stays unchanged.
    fs::write(
        f.dir.path().join(".gitignore"),
        "db.sqlite*\n*.record.lock\n",
    )
    .unwrap();
    f.git(&["add", ".gitignore"]);
    f.git(&["commit", "-qm", "fixture commit after chat"]);
    let stale = f.run(&["restore", "--repo", f.repo()]);
    assert_eq!(stale["git"]["checkpoint_stale"], true);
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert!(count() > before);
    let after = count();
    f.run(&["record", "--repo", f.repo(), "--once"]);
    assert_eq!(count(), after);
    assert_eq!(
        f.run(&["restore", "--repo", f.repo()])["git"]["checkpoint_stale"],
        false
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM commits", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn explicit_selection_imports_waiting_history_in_the_same_restore_call() {
    let f = Fixture::new();
    for task in ["one", "two"] {
        f.run(&["create", task, "--repo", f.repo()]);
    }
    let path = f.transcript("waiting", "waiting-history-marker");
    assert_eq!(
        context(&f.hook("claude", "waiting", Some(&path), "SessionStart"))["status"],
        "selection_required"
    );
    let restored = f.run(&[
        "restore",
        "--repo",
        f.repo(),
        "--task",
        "two",
        "--harness",
        "claude",
        "--session",
        "waiting",
    ]);
    assert!(restored.to_string().contains("waiting-history-marker"));
}

#[test]
fn reconciliation_lock_is_shared_by_foreground_and_one_shot() {
    let f = Fixture::new();
    let path = f.transcript("locked", "lock-marker");
    f.hook("claude", "locked", Some(&path), "SessionStart");
    let lock_path = fs::read_dir(f.dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".record.lock"))
        .unwrap();
    let lock = fs::OpenOptions::new().write(true).open(lock_path).unwrap();
    lock.lock().unwrap();
    assert_eq!(
        f.run(&["record", "--repo", f.repo(), "--once"])["reconciliation_busy"],
        true
    );
    assert_eq!(
        f.run(&["restore", "--repo", f.repo()])["status"],
        "restored"
    );
    drop(lock);
    assert_eq!(f.run(&["record", "--repo", f.repo(), "--once"])["added"], 0);
}

#[test]
fn injected_context_is_preserved_raw_but_not_recursively_indexed() {
    let f = Fixture::new();
    let path = f.transcript("injection", "actual user requirement");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", json!({"type":"message","message":{"role":"custom","customType":"sqnic-context","content":"recursive-private-marker"}})).unwrap();
    writeln!(file, "{}", json!({"type":"attachment","attachment":{"content":"SQnic local handoff. recursive-private-marker"}})).unwrap();
    let restored = context(&f.hook("claude", "injection", Some(&path), "SessionStart"));
    assert!(!restored.to_string().contains("recursive-private-marker"));
    let conn = rusqlite::Connection::open(f.dir.path().join("db.sqlite")).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM events WHERE kind='sqnic_context' AND body='' AND raw LIKE '%recursive-private-marker%'", [], |r|r.get::<_,i64>(0)).unwrap(),2);
}
