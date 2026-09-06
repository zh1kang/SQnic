use crate::{
    model::{clipped, validate_budget, validate_limit},
    store::Store,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    io::Read,
    process::{Command, Stdio},
};

fn git(repo: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .context("run Git")?;
    ensure!(
        output.status.success(),
        "git {}: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub fn sync(store: &mut Store, task: &str) -> Result<Value> {
    let repo = store.repo(task)?;
    git(&repo, &["rev-parse", "--show-toplevel"])?;
    let head = head(&repo)?;
    let hashes = match head.as_deref() {
        Some(hash) => git(&repo, &["rev-list", "--reverse", hash])?,
        None => String::new(),
    };
    let mut records = Vec::new();
    for hash in hashes.lines() {
        let exists: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM commits WHERE task=? AND hash=?)",
            params![task, hash],
            |r| r.get(0),
        )?;
        if exists {
            continue;
        }
        let raw = git(
            &repo,
            &[
                "show",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--root",
                "--format=%H%x00%P%x00%an%x00%aI%x00%B%x00",
                "--numstat",
                hash,
                "--",
            ],
        )?;
        let fields: Vec<_> = raw.splitn(6, '\0').collect();
        ensure!(fields.len() == 6, "unexpected Git metadata format");
        let metadata = json!({"hash":hash,"parents":fields[1].split_whitespace().collect::<Vec<_>>(),"author":fields[2],"authored":fields[3],"message":fields[4].trim_end(),"numstat":fields[5].trim(),"summary_kind":"deterministic Git metadata; purpose and tests are not inferred"});
        let summary = format!(
            "commit {hash}\n{}\nchanges (added/deleted/path):\n{}",
            fields[4].trim_end(),
            fields[5].trim()
        );
        records.push((hash.to_owned(), metadata, summary));
    }
    let snapshot = json!({"head":head,"branch":git(&repo,&["branch","--show-current"])?.trim(),"status":git(&repo,&["status","--porcelain=v1"])?,"captured_unix_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()});
    ensure!(
        self::head(&repo)? == head,
        "HEAD changed during indexing; retry"
    );
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut added = 0;
    for (hash, metadata, summary) in records {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM commits WHERE task=? AND hash=?)",
            params![task, hash],
            |r| r.get(0),
        )?;
        if exists {
            continue;
        }
        tx.execute(
            "INSERT INTO events(task,kind,body,raw) VALUES(?,?,?,?)",
            params![task, "commit", summary, metadata.to_string()],
        )?;
        tx.execute(
            "INSERT INTO commits(task,hash,metadata,summary,event) VALUES(?,?,?,?,?)",
            params![
                task,
                hash,
                metadata.to_string(),
                summary,
                tx.last_insert_rowid()
            ],
        )?;
        added += 1;
    }
    tx.execute(
        "UPDATE tasks SET snapshot=? WHERE id=?",
        params![snapshot.to_string(), task],
    )?;
    tx.execute(
        "INSERT INTO events(task,kind,body,raw) VALUES(?,'checkpoint','Git checkpoint',?)",
        params![task, snapshot.to_string()],
    )?;
    let event = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO checkpoints(task,event,snapshot) VALUES(?,?,?)",
        params![task, event, snapshot.to_string()],
    )?;
    let checkpoint = tx.last_insert_rowid();
    for hash in hashes.lines() {
        tx.execute(
            "INSERT INTO checkpoint_commits(checkpoint,hash) VALUES(?,?)",
            params![checkpoint, hash],
        )?;
    }
    tx.commit()?;
    Ok(
        json!({"checkpoint":checkpoint,"event":event,"added":added,"snapshot":snapshot,"scope":"HEAD ancestry at sync; previously indexed commits remain available"}),
    )
}
pub fn commits(store: &Store, task: &str, offset: usize, limit: usize) -> Result<Value> {
    store.repo(task)?;
    validate_limit(limit)?;
    ensure!(offset <= i32::MAX as usize, "offset too large");
    let mut q=store.conn.prepare("SELECT hash,substr(summary,1,1000),event FROM commits WHERE task=? ORDER BY event DESC LIMIT ? OFFSET ?")?;
    let rows=q.query_map(params![task,limit as i64,offset as i64],|r|Ok(json!({"hash":r.get::<_,String>(0)?,"summary":r.get::<_,String>(1)?,"event":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"next_offset":offset+rows.len(),"commits":rows}))
}
pub fn commit(store: &Store, task: &str, hash: &str, diff: bool, max: usize) -> Result<Value> {
    let repo = store.repo(task)?;
    validate_budget(max)?;
    let raw: String = store
        .conn
        .query_row(
            "SELECT metadata FROM commits WHERE task=? AND hash=?",
            params![task, hash],
            |r| r.get(0),
        )
        .optional()?
        .context("commit not indexed in this task; use full hash")?;
    let metadata: Value = serde_json::from_str(&raw)?;
    let mut q=store.conn.prepare("SELECT id,text,author,created FROM annotations WHERE task=? AND hash=? ORDER BY id DESC LIMIT 10")?;
    let annotations=q.query_map(params![task,hash],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"text":r.get::<_,String>(1)?,"author":r.get::<_,String>(2)?,"created":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let patch = if diff {
        Some(bounded_diff(&repo, hash, max)?)
    } else {
        None
    };
    let links=store.conn.prepare("SELECT event,relation,author FROM evidence_links WHERE task=? AND hash=? ORDER BY event")?.query_map(params![task,hash],|r|Ok(json!({"event":r.get::<_,i64>(0)?,"relation":r.get::<_,String>(1)?,"author":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let reachable:Option<bool>=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM checkpoint_commits cc WHERE cc.checkpoint=c.id AND cc.hash=?) FROM checkpoints c WHERE c.task=? ORDER BY c.id DESC LIMIT 1",params![hash,task],|r|r.get(0)).optional()?;
    Ok(
        json!({"evidence_links":links,"reachable_at_latest_checkpoint":reachable,"metadata":metadata,"annotations":annotations,"diff":patch.as_ref().map(|p|&p.0),"diff_truncated":patch.as_ref().is_some_and(|p|p.1)}),
    )
}
fn head(repo: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["-C", repo, "rev-parse", "--verify", "--quiet", "HEAD"])
        .output()?;
    if output.status.success() {
        return Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()));
    }
    // An unborn symbolic branch is the only valid absent-HEAD state.
    let branch = git(repo, &["symbolic-ref", "-q", "HEAD"])?;
    let reference = Command::new("git")
        .args(["-C", repo, "show-ref", "--verify", "--quiet", branch.trim()])
        .output()?;
    ensure!(
        reference.status.code() == Some(1),
        "HEAD exists but cannot be resolved"
    );
    Ok(None)
}
fn bounded_diff(repo: &str, hash: &str, max: usize) -> Result<(String, bool)> {
    let mut child = Command::new("git")
        .args([
            "-C",
            repo,
            "show",
            "--format=",
            "--no-ext-diff",
            "--no-textconv",
            hash,
            "--",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| -> Result<(String, bool)> {
        let mut bytes = Vec::new();
        let cap = max
            .checked_mul(4)
            .and_then(|n| n.checked_add(4))
            .context("diff budget overflow")?;
        let stdout = child.stdout.take().context("Git stdout unavailable")?;
        stdout.take(cap as u64).read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let truncated = text.chars().count() > max || bytes.len() == cap;
        if bytes.len() == cap {
            // We intentionally stop a potentially large producer after the bounded prefix.
            match child.kill() {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {}
                Err(e) => return Err(e.into()),
            }
        }
        let status = child.wait()?;
        ensure!(
            bytes.len() == cap || status.success(),
            "Git diff failed; verify that the indexed commit still exists locally"
        );
        Ok((clipped(&text, max), truncated))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn annotate(
    store: &mut Store,
    task: &str,
    hash: &str,
    text: &str,
    author: &str,
) -> Result<Value> {
    store.repo(task)?;
    ensure!(
        !text.is_empty() && text.len() <= 32000,
        "annotation must contain 1..32000 bytes"
    );
    ensure!(
        !author.is_empty() && author.len() <= 128,
        "author must contain 1..128 bytes"
    );
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing:Option<i64>=tx.query_row("SELECT id FROM annotations WHERE task=? AND hash=? AND text=? AND author=? ORDER BY id DESC LIMIT 1",params![task,hash,text,author],|r|r.get(0)).optional()?;
    if let Some(id) = existing {
        return Ok(json!({"id":id,"changed":false}));
    }
    tx.execute(
        "INSERT INTO annotations(task,hash,text,author) VALUES(?,?,?,?)",
        params![task, hash, text, author],
    )?;
    let id = tx.last_insert_rowid();
    let raw = json!({"hash":hash,"annotation":text,"author":author,"evidence":"agent-written; verify against commit"});
    tx.execute(
        "INSERT INTO events(task,kind,body,raw) VALUES(?,?,?,?)",
        params![
            task,
            "annotation",
            format!("commit {hash}: {text}"),
            raw.to_string()
        ],
    )?;
    tx.commit()?;
    Ok(json!({"id":id,"changed":true}))
}
