//! Bounded retrieval, explicit provenance and idempotent direct capture.
use crate::{model::clipped, normalize, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashSet;

fn budget(max: usize) -> Result<()> {
    ensure!(
        (512..=100_000).contains(&max),
        "max_bytes must be between 512 and 100000"
    );
    Ok(())
}
fn fits(v: &Value, max: usize) -> bool {
    v.to_string().len() <= max
}
fn push(out: &mut Value, field: &str, item: Value, max: usize) -> bool {
    out[field]
        .as_array_mut()
        .expect("internal response array")
        .push(item);
    if fits(out, max) {
        true
    } else {
        out[field]
            .as_array_mut()
            .expect("internal response array")
            .pop();
        out["omitted"] = json!(true);
        false
    }
}
fn validate_scope(scope: &str) -> Result<()> {
    ensure!(scope.len() <= 256, "scope too long");
    Ok(())
}

pub fn link(
    store: &Store,
    task: &str,
    event: i64,
    hash: &str,
    relation: &str,
    author: &str,
) -> Result<Value> {
    store.repo(task)?;
    ensure!(
        matches!(relation, "supports" | "explains" | "tests"),
        "unknown relation"
    );
    ensure!(
        !author.is_empty() && author.len() <= 128,
        "author must contain 1..128 bytes"
    );
    let exists: bool = store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE task=? AND id=?)",
        params![task, event],
        |r| r.get(0),
    )?;
    ensure!(exists, "event not found in this task");
    let n=store.conn.execute("INSERT INTO evidence_links(task,event,hash,relation,author) VALUES(?,?,?,?,?) ON CONFLICT(task,event,hash,relation) DO NOTHING",params![task,event,hash,relation,author])?;
    Ok(
        json!({"changed":n!=0,"event":event,"hash":hash,"relation":relation,"evidence":"explicit agent assertion; inspect original event before relying on it"}),
    )
}

pub fn enrich(store: &Store, task: &str, event: i64, text: &str, author: &str) -> Result<Value> {
    store.repo(task)?;
    ensure!(
        !text.is_empty() && text.len() <= 8000,
        "enrichment must contain 1..8000 bytes"
    );
    ensure!(
        !author.is_empty() && author.len() <= 128,
        "author must contain 1..128 bytes"
    );
    let exists: bool = store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE task=? AND id=?)",
        params![task, event],
        |r| r.get(0),
    )?;
    ensure!(exists, "event not found in this task");
    let n = store.conn.execute(
        "INSERT OR IGNORE INTO enrichments(task,event,text,author) VALUES(?,?,?,?)",
        params![task, event, text, author],
    )?;
    Ok(json!({"changed":n!=0,"event":event,"derived":true,"original_unchanged":true}))
}

pub fn checkpoints(store: &Store, task: &str, after: i64) -> Result<Value> {
    store.repo(task)?;
    let rows=store.conn.prepare("SELECT id,event,json_extract(snapshot,'$.head'),json_extract(snapshot,'$.branch'),created FROM checkpoints WHERE task=? AND id>? ORDER BY id LIMIT 100")?.query_map(params![task,after],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"event":r.get::<_,i64>(1)?,"head":r.get::<_,Option<String>>(2)?,"branch":r.get::<_,String>(3)?,"created":r.get::<_,String>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"next_after":rows.last().map(|v|&v["id"]),"checkpoints":rows}))
}

pub fn read_many(
    store: &Store,
    task: &str,
    refs: &[String],
    max: usize,
    checkpoint: Option<i64>,
) -> Result<Value> {
    store.repo(task)?;
    budget(max)?;
    ensure!(
        !refs.is_empty() && refs.len() <= 32,
        "supply 1..32 references (event ID or full commit hash prefixed commit:)"
    );
    ensure!(refs.iter().all(|r| r.len() <= 80), "reference too long");
    let mut out = json!({"historical_data":true,"omitted":false,"items":[]});
    let transaction = store.conn.unchecked_transaction()?;
    let watermark = checkpoint
        .map(|id| {
            store
                .conn
                .query_row(
                    "SELECT event FROM checkpoints WHERE task=? AND id=?",
                    params![task, id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .context("checkpoint not found in this task")
        })
        .transpose()?;
    // Reserve a small typed response for every input before adding any evidence.
    for reference in refs {
        out["items"]
            .as_array_mut()
            .context("items")?
            .push(json!({"ref":reference,"status":"budget_exhausted"}));
    }
    ensure!(
        fits(&out, max),
        "budget too small for reference statuses; use fewer references or a larger budget"
    );
    for (i, reference) in refs.iter().enumerate() {
        let result = if let Some(hash) = reference.strip_prefix("commit:") {
            historical_commit(store, task, hash, checkpoint, watermark)
        } else {
            match parse_reference(reference) {
                Some((id, offset)) => store.read_at(task, id, offset, max, watermark),
                _ => Err(anyhow::anyhow!("invalid event reference")),
            }
        };
        let mut item = match result {
            Ok(data) => json!({"ref":reference,"status":"ok","data":data}),
            Err(e) => json!({"ref":reference,"status":"error","error":clipped(&e.to_string(),160)}),
        };
        let placeholder = out["items"][i].clone();
        out["items"][i] = item.clone();
        if !fits(&out, max) {
            // Text pages can shrink safely; metadata and commit objects remain atomic.
            if let Some(text) = item
                .pointer("/data/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                item["status"] = json!("partial");
                let offset = parse_reference(reference).map_or(0, |(_, offset)| offset);
                let chars: Vec<_> = text.chars().collect();
                let (mut low, mut high) = (0, chars.len());
                while low < high {
                    let mid = (low + high).div_ceil(2);
                    item["data"]["text"] = json!(chars[..mid].iter().collect::<String>());
                    item["data"]["next_offset"] = json!(offset + mid);
                    out["items"][i] = item.clone();
                    if fits(&out, max) {
                        low = mid;
                    } else {
                        high = mid - 1;
                    }
                }
                item["data"]["text"] = json!(chars[..low].iter().collect::<String>());
                item["data"]["next_offset"] = json!(offset + low);
                item["status"] = json!("partial");
                out["items"][i] = if low == 0 { placeholder.clone() } else { item };
            }
            if !fits(&out, max) {
                out["items"][i] = placeholder;
            }
            out["omitted"] = json!(true);
        }
    }
    // Changing false to true only reduces the encoded size.
    ensure!(fits(&out, max), "response exceeds budget");
    transaction.commit()?;
    Ok(out)
}

fn terms(query: &str) -> Result<String> {
    ensure!(query.len() <= 4000, "query too long");
    let words: Vec<_> = query
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .take(32)
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    ensure!(!words.is_empty(), "query requires searchable terms");
    Ok(words.join(" OR "))
}

pub fn bundle(
    store: &Store,
    task: &str,
    query: Option<&str>,
    scope: &str,
    checkpoint: Option<i64>,
    max: usize,
) -> Result<Value> {
    store.repo(task)?;
    budget(max)?;
    validate_scope(scope)?;
    let transaction = store.conn.unchecked_transaction()?;
    let cp: Option<(i64, i64, String)> = if let Some(id) = checkpoint {
        Some(
            store
                .conn
                .query_row(
                    "SELECT id,event,snapshot FROM checkpoints WHERE task=? AND id=?",
                    params![task, id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .context("checkpoint not found in this task")?,
        )
    } else {
        store
            .conn
            .query_row(
                "SELECT id,event,snapshot FROM checkpoints WHERE task=? ORDER BY id DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
    };
    let watermark = if checkpoint.is_some() {
        cp.as_ref().map_or(0, |c| c.1)
    } else {
        store.conn.query_row(
            "SELECT coalesce(max(id),0) FROM events WHERE task=?",
            [task],
            |r| r.get(0),
        )?
    };
    let mut out = json!({"historical_data":true,"watermark":watermark,"omitted":false,"state":[],"evidence":[],"checkpoint":null,"budget_unit":"serialized UTF-8 bytes; tokens depend on the reader"});
    let mut q=store.conn.prepare("SELECT n.id,n.event,n.kind,n.key,n.text,n.scope FROM notes n WHERE n.task=? AND (n.scope='' OR n.scope=?) AND n.event<=? AND n.id=(SELECT max(m.id) FROM notes m WHERE m.task=n.task AND m.scope=n.scope AND m.kind=n.kind AND m.key=n.key AND m.event<=?) ORDER BY CASE n.kind WHEN 'constraint' THEN 0 WHEN 'goal' THEN 1 WHEN 'next' THEN 2 ELSE 3 END,n.id DESC LIMIT 101")?;
    let rows=q.query_map(params![task,scope,watermark,watermark],|r|Ok(json!({"revision":r.get::<_,i64>(0)?,"event":r.get::<_,i64>(1)?,"kind":r.get::<_,String>(2)?,"key":r.get::<_,String>(3)?,"text":r.get::<_,String>(4)?,"scope":r.get::<_,String>(5)?})))?;
    for (i, row) in rows.enumerate() {
        let note = row?;
        let required = matches!(note["kind"].as_str(), Some("constraint" | "goal"));
        if required && (i == 100 || !push(&mut out, "state", note.clone(), max)) {
            return Ok(
                json!({"status":"required_state_omitted","historical_data":true,"omitted":true,"watermark":watermark,"required_event":note["event"],"required_revision":note["revision"],"state":[],"evidence":[],"next":"increase max_bytes to include required state before using historical evidence"}),
            );
        }
        if !required {
            if i == 100 {
                out["omitted"] = json!(true);
                break;
            }
            push(&mut out, "state", note, max);
        }
    }
    if let Some((id, event, snapshot)) = &cp {
        let v: Value = serde_json::from_str(snapshot)?;
        let item = json!({"id":id,"event":event,"head":v["head"],"branch":v["branch"],"status":clipped(v["status"].as_str().unwrap_or(""),400),"status_truncated":v["status"].as_str().is_some_and(|s|s.chars().count()>400)});
        out["checkpoint"] = item;
        if !fits(&out, max) {
            out["checkpoint"] = Value::Null;
            out["omitted"] = json!(true);
        }
    }
    let mut candidates = Vec::new();
    if let Some(query) = query {
        let expr = terms(query)?;
        // Keep FTS first: a watermark range otherwise made SQLite scan every task event.
        let mut q=store.conn.prepare("SELECT e.id FROM event_fts CROSS JOIN events e ON e.id=event_fts.rowid LEFT JOIN event_meta m ON m.event=e.id WHERE event_fts MATCH ? AND e.task=? AND e.id<=? AND coalesce(m.scope,'') IN ('',?) AND e.kind NOT IN ('note','checkpoint','sqnic_context') ORDER BY rank LIMIT 24")?;
        candidates.extend(
            q.query_map(
                params![
                    crate::store::scoped_match(&store.conn, task, &expr)?,
                    task,
                    watermark,
                    scope
                ],
                |r| r.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        // Derived search keys are excluded from historical snapshots: they may have been written later.
        if checkpoint.is_none() {
            candidates.extend(store.conn.prepare("SELECT x.event FROM enrichment_fts CROSS JOIN enrichments x ON x.id=enrichment_fts.rowid LEFT JOIN event_meta m ON m.event=x.event WHERE enrichment_fts MATCH ? AND x.task=? AND x.event<=? AND coalesce(m.scope,'') IN ('',?) ORDER BY rank LIMIT 12")?.query_map(params![expr,task,watermark,scope],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
    } else {
        candidates.extend(store.conn.prepare("SELECT e.id FROM events e LEFT JOIN event_meta m ON m.event=e.id WHERE e.task=? AND e.id<=? AND coalesce(m.scope,'') IN ('',?) AND e.kind NOT IN ('note','checkpoint','sqnic_context') ORDER BY e.id DESC LIMIT 24")?.query_map(params![task,watermark,scope],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    let mut seen = HashSet::new();
    for id in candidates {
        if !seen.insert(id) {
            continue;
        }
        // At most one hop: explicit parent and matched call/result; never all sibling branches.
        let metadata = historical_metadata(store, task, id, watermark)?;
        let mut group = vec![id];
        if let Some(parents) = metadata["parent_events"]
            .as_array()
            .filter(|p| p.len() == 1)
        {
            group.extend(parents.iter().filter_map(Value::as_i64));
        }
        if let Some(refs) = metadata["tools"].as_array() {
            for tool in refs {
                if let Some(peers) = tool["related_events"].as_array().filter(|p| p.len() == 1) {
                    group.extend(peers.iter().filter_map(Value::as_i64));
                }
            }
        }
        for related in group {
            if related != id && !seen.insert(related) {
                continue;
            }
            let row=store.conn.query_row("SELECT e.kind,substr(e.body,1,600),e.source,e.line,length(e.raw) FROM events e LEFT JOIN event_meta m ON m.event=e.id WHERE e.task=? AND e.id=? AND e.id<=? AND e.kind != 'sqnic_context' AND coalesce(m.scope,'') IN ('',?)",params![task,related,watermark,scope],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,i64>(4)?))).optional()?;
            if let Some((kind, text, source, line, total)) = row {
                if kind == "commit"
                    && let Some((cp_id, _, _)) = &cp
                {
                    let reachable:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM commits c JOIN checkpoint_commits cc ON cc.hash=c.hash WHERE c.task=? AND c.event=? AND cc.checkpoint=?)",params![task,related,cp_id],|r|r.get(0))?;
                    if !reachable {
                        continue;
                    }
                }
                let meta = historical_metadata(store, task, related, watermark)?;
                let item = json!({"event":related,"kind":kind,"excerpt":text,"source":source,"line":line,"raw_chars":total,"metadata":meta,"derived":if checkpoint.is_none(){enrichments(store,task,related)?}else{json!([])},"excerpt_only":true});
                push(&mut out, "evidence", item, max);
            }
        }
    }
    // The candidate window is bounded; never imply complete search coverage.
    out["candidate_window_limited"] = json!(true);
    if !fits(&out, max) {
        out.as_object_mut()
            .context("bundle")?
            .remove("candidate_window_limited");
        out["omitted"] = json!(true);
    }
    transaction.commit()?;
    Ok(out)
}

fn historical_metadata(store: &Store, task: &str, id: i64, watermark: i64) -> Result<Value> {
    let mut meta = normalize::metadata_at(&store.conn, task, id, watermark)?;
    if meta["episode"].as_i64().is_some_and(|id| id > watermark) {
        meta["episode"] = Value::Null;
    }
    if let Some(parents) = meta["parent_events"].as_array_mut() {
        parents.retain(|v| v.as_i64().is_some_and(|id| id <= watermark));
    }
    if let Some(refs) = meta["tools"].as_array_mut() {
        for tool in refs {
            if let Some(peers) = tool["related_events"].as_array_mut() {
                peers.retain(|v| v.as_i64().is_some_and(|id| id <= watermark));
            }
        }
    }
    Ok(meta)
}

fn parse_reference(reference: &str) -> Option<(i64, usize)> {
    let (id, offset) = reference.split_once('@').unwrap_or((reference, "0"));
    let id = id.parse::<i64>().ok()?;
    let offset = offset.parse::<usize>().ok()?;
    (id > 0 && offset <= i32::MAX as usize).then_some((id, offset))
}

pub fn enrichments(store: &Store, task: &str, event: i64) -> Result<Value> {
    let rows=store.conn.prepare("SELECT id,substr(text,1,600),author,length(text),created FROM enrichments WHERE task=? AND event=? ORDER BY id DESC LIMIT 3")?.query_map(params![task,event],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"text":r.get::<_,String>(1)?,"author":r.get::<_,String>(2)?,"truncated":r.get::<_,i64>(3)?>600,"created":r.get::<_,String>(4)?,"agent_written":true})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(rows))
}

fn historical_commit(
    store: &Store,
    task: &str,
    hash: &str,
    checkpoint: Option<i64>,
    watermark: Option<i64>,
) -> Result<Value> {
    if let Some(watermark) = watermark {
        let exists: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM commits WHERE task=? AND hash=? AND event<=?)",
            params![task, hash, watermark],
            |r| r.get(0),
        )?;
        ensure!(exists, "commit not available at this checkpoint");
    }
    let mut value = crate::git::commit(store, task, hash, false, 256)?;
    if let Some(checkpoint) = checkpoint {
        value["annotations"] = json!([]);
        value["evidence_links"] = json!([]);
        value["derived_omitted_for_historical_read"] = json!(true);
        value
            .as_object_mut()
            .context("commit response")?
            .remove("reachable_at_latest_checkpoint");
        let reachable: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM checkpoint_commits WHERE checkpoint=? AND hash=?)",
            params![checkpoint, hash],
            |r| r.get(0),
        )?;
        value["reachable_at_selected_checkpoint"] = json!(reachable);
    }
    Ok(value)
}
