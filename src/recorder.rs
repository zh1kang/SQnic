//! Recoverable reconciliation of explicit native transcript paths.
use crate::{automatic, import, model::Format, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn approved_path(path: &Path, repo: &str, harness: &str) -> Result<()> {
    let mut roots = vec![std::path::PathBuf::from(repo)];
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let (variable, default, children): (&str, &str, &[&str]) = match harness {
        "codex" => ("CODEX_HOME", ".codex", &["sessions", "archived_sessions"]),
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude", &["projects"]),
        "pi" => ("PI_CODING_AGENT_DIR", ".pi/agent", &["sessions"]),
        _ => anyhow::bail!("unsupported automatic transcript source"),
    };
    let base = std::env::var_os(variable)
        .map(std::path::PathBuf::from)
        .or_else(|| home.map(|p| p.join(default)));
    if let Some(base) = base {
        roots.extend(children.iter().map(|child| base.join(child)));
    }
    ensure!(
        roots
            .iter()
            .filter_map(|root| std::fs::canonicalize(root).ok())
            .any(|root| path.starts_with(root)),
        "transcript is outside the project and approved harness history roots; use explicit import for custom exports"
    );
    Ok(())
}

pub(crate) fn verify(
    file: &mut File,
    path: &Path,
    repo: &str,
    harness: &str,
    native: &str,
) -> Result<()> {
    approved_path(path, repo, harness)?;
    ensure!(
        file.metadata()?.is_file(),
        "transcript is not a regular file"
    );
    // A native identity header must be present before an automatic import is allowed.
    let reader = BufReader::new(file.take(1024 * 1024));
    for line in reader.lines().take(128) {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let p = if v["type"] == "session_meta" {
            &v["payload"]
        } else {
            &v
        };
        let id = p
            .get("sessionId")
            .or_else(|| p.get("session_id"))
            .or_else(|| p.get("id"))
            .and_then(Value::as_str);
        let cwd = p.get("cwd").and_then(Value::as_str);
        if let Some(cwd) = cwd {
            let session_matches = id == Some(native)
                || (harness == "claude"
                    && path.file_stem().and_then(|s| s.to_str()) == Some(native));
            if session_matches {
                ensure!(
                    automatic::root(cwd)? == repo,
                    "transcript belongs to another worktree"
                );
                return Ok(());
            }
        }
    }
    anyhow::bail!("native session/worktree identity not yet verified in transcript header")
}

pub fn reconcile(store: &mut Store, repo: &str, foreground: bool) -> Result<Value> {
    if !automatic::enabled(store, repo)? {
        return Ok(json!({"paused":true}));
    }
    // An OS lock covers every reconciliation entry point and is released on crash.
    // The separate expiring lease avoids launching idle duplicate worker processes.
    let db = store
        .conn
        .path()
        .context("automatic recording requires a file database")?;
    let lock_path =
        Path::new(db).with_extension(format!("{}.record.lock", automatic::digest(repo)));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Ok(json!({"reconciliation_busy":true,"added":0}));
        }
        Err(e) => return Err(e.into()),
    }
    let scope = automatic::branch(repo)?;
    let files:Vec<(i64,String,String,Option<String>)>=store.conn.prepare("SELECT a.id,a.task,f.path,f.stamp FROM auto_files f JOIN auto_sessions a ON a.id=f.session WHERE a.repo=? AND a.excluded=0 AND a.task IS NOT NULL AND a.branch=? ORDER BY coalesce(f.checked,0),a.id,f.path LIMIT 256")?.query_map(params![repo,scope],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut changed = 0;
    let mut deferred = 0;
    let mut failures = 0;
    let mut remaining = 8 * 1024 * 1024;
    for (id, task, path, old_stamp) in files {
        if !automatic::enabled(store, repo)? {
            break;
        }
        let result = (|| -> Result<Option<(String, Value)>> {
            let metadata = std::fs::metadata(&path)?;
            ensure!(metadata.is_file(), "transcript must be a regular file");
            let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
            let canonical = std::fs::canonicalize(&path)?;
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                format!("{}:{}", metadata.dev(), metadata.ino())
            };
            #[cfg(not(unix))]
            let identity = format!("{:?}", metadata.created().ok());
            let stamp = format!(
                "{}:{modified}:{identity}:{}",
                metadata.len(),
                canonical.display()
            );
            if old_stamp.as_ref() == Some(&stamp) {
                store.conn.execute(
                    "UPDATE auto_files SET error=NULL,checked=? WHERE session=? AND path=?",
                    params![automatic::now(), id, path],
                )?;
                return Ok(None);
            }
            if foreground && metadata.len() > remaining {
                deferred += 1;
                store.conn.execute("UPDATE auto_files SET error='pending background reconciliation',checked=? WHERE session=? AND path=?",params![automatic::now(),id,path])?;
                return Ok(None);
            }
            remaining = remaining.saturating_sub(metadata.len());
            let value = import::import_automatic(
                store,
                &task,
                canonical
                    .to_str()
                    .context("transcript path must be UTF-8")?,
                Format::Jsonl,
                id,
            )?;
            Ok(Some((stamp, value)))
        })();
        match result {
            Ok(Some((stamp, value))) => {
                changed += value["added"].as_u64().unwrap_or(0);
                store.conn.execute("UPDATE auto_files SET stamp=?,error=NULL,pending=?,checked=? WHERE session=? AND path=?",params![stamp,value["pending_bytes"].as_i64().unwrap_or(0),automatic::now(),id,path])?;
            }
            Ok(None) => {}
            Err(e) => {
                failures += 1;
                store.conn.execute(
                    "UPDATE auto_files SET error=?,checked=? WHERE session=? AND path=?",
                    params![format!("{e:#}"), automatic::now(), id, path],
                )?;
            }
        }
    }
    if !foreground {
        let tasks=store.conn.prepare("SELECT task FROM auto_sessions WHERE repo=? AND branch=? AND excluded=0 AND task IS NOT NULL AND seen>=unixepoch()-120 GROUP BY task ORDER BY max(seen) DESC LIMIT 64")?.query_map(params![repo,scope],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for task in tasks {
            let latest: Option<String> = store
                .conn
                .query_row(
                    "SELECT snapshot FROM checkpoints WHERE task=? ORDER BY id DESC LIMIT 1",
                    [&task],
                    |r| r.get(0),
                )
                .optional()?;
            let outcome = (|| -> Result<()> {
                let current = crate::git::snapshot(repo)?;
                let previous = latest
                    .as_deref()
                    .map(serde_json::from_str::<Value>)
                    .transpose()?;
                let differs = previous.as_ref().is_none_or(|p| {
                    p["head"] != current["head"]
                        || p["branch"] != current["branch"]
                        || p["status"] != current["status"]
                });
                if differs {
                    crate::git::sync_automatic(store, &task)?;
                }
                Ok(())
            })();
            store.conn.execute(
                "UPDATE auto_sessions SET error=? WHERE repo=? AND task=? AND excluded=0",
                params![
                    outcome.err().map(|e| format!("Git checkpoint: {e:#}")),
                    repo,
                    task
                ],
            )?;
        }
    }
    Ok(
        json!({"files_per_pass_limit":256,"added":changed,"deferred_files":deferred,"failed_files":failures,"foreground_byte_scan_limit":if foreground {Some(8*1024*1024)}else{None}}),
    )
}

pub fn run(store: &mut Store, repo: &str, once: bool) -> Result<Value> {
    if once {
        return reconcile(store, repo, false);
    }
    if !automatic::enabled(store, repo)? {
        return Ok(json!({"paused":true}));
    }
    let token = automatic::digest(&format!("{}:{:?}", std::process::id(), SystemTime::now()));
    let acquired=store.conn.execute("INSERT INTO auto_leases(repo,token,expires) VALUES(?,?,?) ON CONFLICT(repo) DO UPDATE SET token=excluded.token,expires=excluded.expires WHERE auto_leases.expires<=?",params![repo,token,automatic::now()+30,automatic::now()])?;
    if acquired == 0 {
        return Ok(json!({"recorder_already_running":true}));
    }
    let mut activity = Instant::now();
    let mut last_seen = 0;
    loop {
        if !automatic::enabled(store, repo)? {
            break;
        }
        let n = store.conn.execute(
            "UPDATE auto_leases SET expires=? WHERE repo=? AND token=?",
            params![automatic::now() + 30, repo, token],
        )?;
        if n == 0 {
            break;
        }
        let result = reconcile(store, repo, false)?;
        let seen: Option<i64> = store
            .conn
            .query_row(
                "SELECT max(seen) FROM auto_sessions WHERE repo=?",
                [repo],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if result["added"].as_u64().unwrap_or(0) > 0 || seen.unwrap_or(0) != last_seen {
            activity = Instant::now();
            last_seen = seen.unwrap_or(0);
        }
        if activity.elapsed() > Duration::from_secs(120) {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    store.conn.execute(
        "DELETE FROM auto_leases WHERE repo=? AND token=?",
        params![repo, token],
    )?;
    Ok(json!({"recorder_stopped":true}))
}
