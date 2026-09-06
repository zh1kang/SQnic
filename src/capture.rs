//! Atomic direct capture without rereading external history.
use crate::{import, normalize, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub fn append(store: &mut Store, task: &str, key: &str, raw: &str) -> Result<Value> {
    store.repo(task)?;
    ensure!(
        !key.is_empty() && key.len() <= 256,
        "capture key must contain 1..256 bytes"
    );
    ensure!(
        raw.len() <= 512_000,
        "capture record exceeds 512000 bytes; use file import"
    );
    let v: Value = serde_json::from_str(raw).context("capture requires one JSON object")?;
    ensure!(v.is_object(), "capture requires one JSON object");
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let old:Option<(i64,String)>=tx.query_row("SELECT c.event,e.raw FROM captures c JOIN events e ON e.id=c.event WHERE c.task=? AND c.key=?",params![task,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((event, previous)) = old {
        ensure!(
            raw == previous,
            "capture key conflict: existing record differs"
        );
        return Ok(json!({"event":event,"added":false,"prefix_verified_bytes":0}));
    }
    let (kind, body) = import::normalize(&v);
    tx.execute(
        "INSERT INTO events(task,kind,body,raw) VALUES(?,?,?,?)",
        params![task, kind, body, raw],
    )?;
    let event = tx.last_insert_rowid();
    normalize::record(&tx, event, None, &v)?;
    tx.execute(
        "INSERT INTO captures(task,key,event) VALUES(?,?,?)",
        params![task, key, event],
    )?;
    tx.commit()?;
    Ok(json!({"event":event,"added":true,"prefix_verified_bytes":0}))
}
