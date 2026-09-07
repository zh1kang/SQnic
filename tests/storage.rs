use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("context.sqlite3");
        let fixture = Self { dir, db };
        fixture.run(&[
            "create",
            "alpha",
            "--repo",
            fixture.dir.path().to_str().unwrap(),
        ]);
        fixture
    }

    fn output(&self, db: &Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_sqnic"))
            .arg("--db")
            .arg(db)
            .args(args)
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Value {
        let output = self.output(&self.db, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn online_backup_copies_committed_wal_content() {
    let fixture = Fixture::new();
    let conn = Connection::open(&fixture.db).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    conn.execute(
        "INSERT INTO events(task,kind,body,raw) VALUES('alpha','message','live wal marker','live wal marker')",
        [],
    )
    .unwrap();
    assert!(fixture.db.with_file_name("context.sqlite3-wal").exists());
    let backup = fixture.dir.path().join("snapshot.sqlite3");
    fixture.run(&["backup", backup.to_str().unwrap()]);
    let output = fixture.output(&backup, &["history", "alpha"]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.to_string().contains("live wal marker"));
}

#[test]
fn restore_refuses_existing_destination_and_validates_new_copy() {
    let fixture = Fixture::new();
    fixture.run(&[
        "update",
        "alpha",
        "--kind",
        "goal",
        "--text",
        "restore marker",
    ]);
    let backup = fixture.dir.path().join("snapshot.sqlite3");
    fixture.run(&["backup", backup.to_str().unwrap()]);
    let restored = fixture.dir.path().join("restored.sqlite3");
    fixture.run(&[
        "restore-backup",
        backup.to_str().unwrap(),
        "--output",
        restored.to_str().unwrap(),
    ]);
    let output = fixture.output(&restored, &["resume", "alpha"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("restore marker"));

    let occupied = fixture.dir.path().join("occupied.sqlite3");
    fs::write(&occupied, b"existing").unwrap();
    let refused = fixture.output(
        &fixture.db,
        &[
            "restore-backup",
            backup.to_str().unwrap(),
            "--output",
            occupied.to_str().unwrap(),
        ],
    );
    assert!(!refused.status.success());

    let sidecar_destination = fixture.dir.path().join("sidecar.sqlite3");
    fs::write(
        sidecar_destination.with_file_name("sidecar.sqlite3-wal"),
        b"orphan",
    )
    .unwrap();
    let sidecar_refused = fixture.output(
        &fixture.db,
        &[
            "restore-backup",
            backup.to_str().unwrap(),
            "--output",
            sidecar_destination.to_str().unwrap(),
        ],
    );
    assert!(!sidecar_refused.status.success());
}

#[test]
fn restore_rejects_an_empty_sqlite_database() {
    let fixture = Fixture::new();
    let empty = fixture.dir.path().join("empty.sqlite3");
    Connection::open(&empty).unwrap();
    let output = fixture.dir.path().join("restored.sqlite3");
    let result = fixture.output(
        &fixture.db,
        &[
            "restore-backup",
            empty.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert!(!result.status.success());
    assert!(!output.exists());
}

#[test]
fn delete_task_isolates_other_tasks_and_removes_fts_rows() {
    let fixture = Fixture::new();
    fixture.run(&[
        "create",
        "beta",
        "--repo",
        fixture.dir.path().to_str().unwrap(),
    ]);
    fixture.run(&[
        "update",
        "alpha",
        "--kind",
        "goal",
        "--text",
        "private alpha marker",
    ]);
    fixture.run(&[
        "update",
        "beta",
        "--kind",
        "goal",
        "--text",
        "keep beta marker",
    ]);
    {
        let conn = Connection::open(&fixture.db).unwrap();
        conn.execute(
            "INSERT INTO auto_projects(repo) VALUES(?)",
            [fixture.dir.path().to_str().unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO auto_sessions(repo,harness,native_id,task,branch,seen,error) VALUES(?,?,?,?,?,?,?)",
            rusqlite::params![fixture.dir.path().to_str().unwrap(), "codex", "private-session", "alpha", "main", 1, "private transcript path"],
        )
        .unwrap();
        let session = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO auto_files(session,path,error) VALUES(?,?,?)",
            rusqlite::params![
                session,
                "/private/transcript.jsonl",
                "private transcript path"
            ],
        )
        .unwrap();
    }
    fixture.run(&["delete-task", "alpha", "--confirm", "alpha"]);
    assert!(
        !fixture
            .output(&fixture.db, &["resume", "alpha"])
            .status
            .success()
    );
    assert!(
        String::from_utf8_lossy(&fixture.output(&fixture.db, &["resume", "beta"]).stdout)
            .contains("keep beta marker")
    );
    assert_eq!(
        fixture.run(&["search", "beta", "marker"])["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let conn = Connection::open(&fixture.db).unwrap();
    conn.execute(
        "INSERT INTO event_fts(event_fts) VALUES('integrity-check')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO enrichment_fts(enrichment_fts) VALUES('integrity-check')",
        [],
    )
    .unwrap();
    let session: (bool, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT excluded,task,error FROM auto_sessions WHERE native_id='private-session'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(session, (true, None, None));
    let auto_files: i64 = conn
        .query_row("SELECT count(*) FROM auto_files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(auto_files, 0);
}

#[test]
fn read_only_mode_refuses_backup_and_restore_output_writes() {
    let fixture = Fixture::new();
    let backup = fixture.dir.path().join("backup.sqlite3");
    fixture.run(&["backup", backup.to_str().unwrap()]);
    for args in [
        vec!["--read-only", "backup"],
        vec![
            "--read-only",
            "restore-backup",
            backup.to_str().unwrap(),
            "--output",
        ],
    ] {
        let destination = fixture.dir.path().join("must-not-exist.sqlite3");
        let mut command = args;
        command.push(destination.to_str().unwrap());
        assert!(!fixture.output(&fixture.db, &command).status.success());
        assert!(!destination.exists());
    }
}

#[cfg(unix)]
#[test]
fn backup_refuses_dangling_destination_and_sidecar_symlinks() {
    let fixture = Fixture::new();
    let destination = fixture.dir.path().join("snapshot.sqlite3");
    let missing = fixture.dir.path().join("missing");
    for path in [
        destination.clone(),
        destination.with_file_name("snapshot.sqlite3-wal"),
    ] {
        std::os::unix::fs::symlink(&missing, &path).unwrap();
        let result = fixture.output(&fixture.db, &["backup", destination.to_str().unwrap()]);
        assert!(!result.status.success());
        assert!(!destination.exists());
        assert!(!missing.exists());
        assert!(path.is_symlink());
        fs::remove_file(path).unwrap();
    }
}
