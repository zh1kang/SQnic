use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Creates an online backup without interrupting readers or writers on `source`.
///
/// The destination must not already exist. A temporary file in the destination
/// directory is validated before it is atomically linked into place.
pub fn backup(source: &Connection, destination: &Path) -> Result<Value> {
    ensure_destination_is_new(destination)?;
    let temporary = temporary_path(destination);
    create_parent(destination)?;
    let result = (|| -> Result<()> {
        create_private_file(&temporary)?;
        let mut target = Connection::open(&temporary).context("open backup destination")?;
        {
            let backup = Backup::new(source, &mut target).context("start online sqlite backup")?;
            backup
                .run_to_completion(128, Duration::from_millis(10), None)
                .context("complete online sqlite backup")?;
        }
        target.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        target.close().map_err(|(_, error)| error)?;
        validate_database(&temporary)?;
        publish_new_file(&temporary, destination).context("publish sqlite backup")?;
        Ok(())
    })();
    if result.is_err() {
        remove_temporary_files(&temporary);
    }
    result?;
    Ok(json!({"backup": destination, "validated": true}))
}

/// Restores a backup into a new database path and refuses to overwrite any path.
pub fn restore_backup(backup_path: &Path, destination: &Path) -> Result<Value> {
    ensure!(backup_path.is_file(), "backup path is not a file");
    ensure_destination_is_new(destination)?;
    create_parent(destination)?;
    let temporary = temporary_path(destination);
    let result = (|| -> Result<()> {
        create_private_file(&temporary)?;
        let source = Connection::open_with_flags(backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("open sqlite backup read-only")?;
        validate_database_connection(&source)?;
        let mut target = Connection::open(&temporary).context("open restore destination")?;
        {
            let restore = Backup::new(&source, &mut target).context("start sqlite restore")?;
            restore
                .run_to_completion(128, Duration::from_millis(10), None)
                .context("complete sqlite restore")?;
        }
        target.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        target.close().map_err(|(_, error)| error)?;
        validate_database(&temporary)?;
        publish_new_file(&temporary, destination).context("publish restored sqlite database")?;
        Ok(())
    })();
    if result.is_err() {
        remove_temporary_files(&temporary);
    }
    result?;
    Ok(json!({"restored": destination, "validated": true}))
}

/// Deletes one task and all task-owned rows while preserving other tasks.
pub fn delete_task(conn: &mut Connection, task: &str, confirmation: &str) -> Result<Value> {
    ensure!(!task.is_empty(), "task cannot be empty");
    ensure!(
        task == confirmation,
        "--confirm must exactly match the task ID"
    );
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)",
        [task],
        |row| row.get(0),
    )?;
    ensure!(exists, "unknown task {task:?}");

    // Keep session identities as an exclusion record so a later reconciliation
    // cannot attach the old native transcript to a different task.
    tx.execute(
        "DELETE FROM auto_files WHERE session IN (SELECT id FROM auto_sessions WHERE task=?)",
        [task],
    )?;
    tx.execute(
        "UPDATE auto_sessions SET excluded=1, task=NULL, error=NULL WHERE task=?",
        [task],
    )?;

    tx.execute(
        "INSERT INTO event_fts(event_fts,rowid,body,task)
         SELECT 'delete',id,body,task FROM events WHERE task=?",
        [task],
    )?;
    tx.execute(
        "INSERT INTO enrichment_fts(enrichment_fts,rowid,text)
         SELECT 'delete',id,text FROM enrichments WHERE task=?",
        [task],
    )?;

    for sql in [
        "DELETE FROM evidence_links WHERE task=?",
        "DELETE FROM annotations WHERE task=?",
        "DELETE FROM checkpoint_commits WHERE checkpoint IN (SELECT id FROM checkpoints WHERE task=?)",
        "DELETE FROM captures WHERE task=?",
        "DELETE FROM tool_refs WHERE event IN (SELECT id FROM events WHERE task=?)",
        "DELETE FROM event_meta WHERE event IN (SELECT id FROM events WHERE task=?)",
        "DELETE FROM enrichments WHERE task=?",
        "DELETE FROM notes WHERE task=?",
        "DELETE FROM checkpoints WHERE task=?",
        "DELETE FROM commits WHERE task=?",
        "DELETE FROM events WHERE task=?",
        "DELETE FROM sources WHERE task=?",
        "DELETE FROM tasks WHERE id=?",
    ] {
        tx.execute(sql, [task])?;
    }
    tx.execute(
        "INSERT INTO event_fts(event_fts) VALUES('integrity-check')",
        [],
    )?;
    tx.execute(
        "INSERT INTO enrichment_fts(enrichment_fts) VALUES('integrity-check')",
        [],
    )?;
    let foreign_keys: i64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    ensure!(
        foreign_keys == 0,
        "foreign key validation failed after task deletion"
    );
    tx.commit()?;
    Ok(json!({"deleted": task, "foreign_keys_valid": true}))
}

fn ensure_destination_is_new(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty(),
        "destination path cannot be empty"
    );
    ensure!(
        !path_entry_exists(path)?,
        "destination already exists; refusing to overwrite it"
    );
    for sidecar in sidecar_paths(path) {
        ensure!(
            !path_entry_exists(&sidecar)?,
            "destination has an existing sqlite sidecar; refusing to use it"
        );
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if !parent.exists() {
            fs::create_dir_all(parent).context("create destination directory")?;
        }
        ensure!(parent.is_dir(), "destination parent is not a directory");
    }
    Ok(())
}

fn create_private_file(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map(|_| ())
        .context("create private sqlite temporary file")
}

fn publish_new_file(temporary: &Path, destination: &Path) -> Result<()> {
    // A hard link succeeds atomically only when the destination is absent, so
    // a concurrent creator cannot turn publication into an overwrite.
    for sidecar in sidecar_paths(temporary) {
        ensure!(
            !path_entry_exists(&sidecar)?,
            "temporary sqlite sidecar prevents publication"
        );
    }
    ensure_destination_is_new(destination)?;
    fs::hard_link(temporary, destination).context("link validated sqlite file")?;
    fs::remove_file(temporary).context("remove sqlite temporary file")?;
    Ok(())
}

fn sidecar_paths(path: &Path) -> [PathBuf; 2] {
    ["-wal", "-shm"].map(|suffix| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    })
}

fn remove_temporary_files(path: &Path) {
    let _ = fs::remove_file(path);
    for sidecar in sidecar_paths(path) {
        let _ = fs::remove_file(sidecar);
    }
}

fn temporary_path(destination: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!(
        ".{}.{}.{}.tmp",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        std::process::id(),
        stamp
    );
    destination.with_file_name(name)
}

fn validate_database(path: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("open database for validation")?;
    validate_database_connection(&connection)
}

fn validate_database_connection(connection: &Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    ensure!(
        (1..=4).contains(&version),
        "unsupported sqlite schema version {version}"
    );
    for table in ["tasks", "sources", "events", "notes", "event_fts"] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=? AND type IN ('table','view'))",
            [table],
            |row| row.get(0),
        )?;
        ensure!(exists, "sqlite database is missing required table {table}");
    }
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    ensure!(
        integrity == "ok",
        "sqlite integrity check failed: {integrity}"
    );
    let foreign_keys: i64 =
        connection.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    ensure!(foreign_keys == 0, "sqlite foreign key check failed");
    Ok(())
}
