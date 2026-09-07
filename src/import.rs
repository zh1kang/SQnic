use crate::{model::Format, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek},
};

const MAX_RECORD: u64 = 16 * 1024 * 1024;

pub fn import(store: &mut Store, task: &str, path: &str, format: Format) -> Result<Value> {
    import_guarded(store, task, path, format, None)
}
pub fn import_automatic(
    store: &mut Store,
    task: &str,
    path: &str,
    format: Format,
    session: i64,
) -> Result<Value> {
    import_guarded(store, task, path, format, Some(session))
}
fn import_guarded(
    store: &mut Store,
    task: &str,
    path: &str,
    format: Format,
    session: Option<i64>,
) -> Result<Value> {
    store.repo(task)?;
    let path = std::fs::canonicalize(path).context("resolve history path")?;
    let name = path.to_str().context("history path must be UTF-8")?;
    let mut file = File::open(&path)?;
    let before = file.metadata()?;
    ensure!(before.is_file(), "history must be a regular file");
    if let Some(session) = session {
        let (repo, harness, native): (String, String, String) = store.conn.query_row(
            "SELECT repo,harness,native_id FROM auto_sessions WHERE id=?",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        // Validate the same open descriptor that the importer consumes.
        crate::recorder::verify(&mut file, &path, &repo, &harness, &native)?;
    }

    let old: Option<(i64, String, i64, i64, String)> = store
        .conn
        .query_row(
            "SELECT id,format,offset,line,digest FROM sources WHERE task=? AND path=?",
            params![task, name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let requested = if format == Format::Auto {
        if matches!(
            path.extension().and_then(|x| x.to_str()),
            Some("md" | "txt")
        ) {
            Format::Text
        } else {
            Format::Jsonl
        }
    } else {
        format
    };
    let selected = if let Some(ref old) = old {
        let previous: Format = serde_json::from_str(&old.1)?;
        ensure!(
            format == Format::Auto || previous == requested,
            "source format cannot change on refresh"
        );
        previous
    } else {
        requested
    };
    let offset = old.as_ref().map_or(0, |x| x.2 as u64);
    ensure!(
        before.len() >= offset,
        "history was truncated; import a new snapshot path"
    );
    let mut hasher = Sha256::new();
    hash_prefix(&mut file, offset, &mut hasher)?;
    if let Some(ref old) = old {
        ensure!(
            digest_hex(hasher.clone()) == old.4,
            "history prefix changed; import a new snapshot path"
        );
        if selected == Format::Text {
            ensure!(
                before.len() == offset,
                "text snapshot changed; export to a new path"
            );
        }
    }
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(session) = session {
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM auto_sessions a JOIN auto_projects p ON p.repo=a.repo WHERE a.id=? AND a.task=? AND a.excluded=0 AND p.enabled=1)",params![session,task],|r|r.get(0))?;
        ensure!(active, "automatic recording is paused or excluded");
    }
    // The cursor is checked after the write lock to prevent two importers from appending the same batch.
    let current: Option<i64> = tx
        .query_row(
            "SELECT offset FROM sources WHERE task=? AND path=?",
            params![task, name],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        current == old.as_ref().map(|x| x.2),
        "source changed during import; retry"
    );
    tx.execute("INSERT INTO sources(task,path,format,digest) VALUES(?,?,?,?) ON CONFLICT(task,path) DO NOTHING",params![task,name,serde_json::to_string(&selected)?,digest_hex(Sha256::new())])?;
    let source: i64 = tx.query_row(
        "SELECT id FROM sources WHERE task=? AND path=?",
        params![task, name],
        |r| r.get(0),
    )?;
    let mut line = old.as_ref().map_or(0, |x| x.3);
    let mut consumed = offset;
    let mut added = 0;
    let mut reader = BufReader::new(file.take(before.len() - offset));
    loop {
        let mut bytes = Vec::new();
        let n = reader
            .by_ref()
            .take(MAX_RECORD + 1)
            .read_until(b'\n', &mut bytes)?;
        if n == 0 {
            break;
        }
        ensure!(n as u64 <= MAX_RECORD, "record {} exceeds 16 MiB", line + 1);
        if selected != Format::Text && bytes.last() != Some(&b'\n') {
            break;
        }
        let raw = std::str::from_utf8(&bytes)
            .with_context(|| format!("invalid UTF-8 at line {}", line + 1))?;
        let mut parsed = Value::Null;
        let (kind, body) = if selected == Format::Text {
            ("text".to_owned(), raw.to_owned())
        } else if raw.trim().is_empty() {
            ("blank".to_owned(), String::new())
        } else {
            let value: Value = serde_json::from_str(raw)
                .with_context(|| format!("invalid JSON at line {}", line + 1))?;
            ensure!(value.is_object(), "line {} must be a JSON object", line + 1);
            let normalized = normalize(&value);
            parsed = value;
            normalized
        };
        line += 1;
        tx.execute(
            "INSERT INTO events(task,source,line,kind,body,raw) VALUES(?,?,?,?,?,?)",
            params![task, source, line, kind, body, raw],
        )?;
        let event = tx.last_insert_rowid();
        crate::normalize::record(&tx, event, Some(source), &parsed)?;
        if let Some(session) = session {
            tx.execute("UPDATE event_meta SET scope=(SELECT branch FROM auto_sessions WHERE id=?) WHERE event=? AND scope=''",params![session,event])?;
        }
        hasher.update(&bytes);
        consumed += n as u64;
        added += 1;
    }
    let file = reader.into_inner().into_inner();
    let after = file.metadata()?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "history changed while reading; retry when the current write finishes"
    );
    tx.execute(
        "UPDATE sources SET offset=?,line=?,digest=?,updated=CURRENT_TIMESTAMP WHERE id=?",
        params![i64::try_from(consumed)?, line, digest_hex(hasher), source],
    )?;
    tx.commit()?;
    Ok(
        json!({"source":source,"path":name,"format":selected,"added":added,"consumed_bytes":consumed,"new_bytes":consumed-offset,"prefix_verified_bytes":offset,"pending_bytes":before.len()-consumed,"coverage":"available file records only; supported native links are normalized, originals retained"}),
    )
}

fn hash_prefix(file: &mut File, mut remaining: u64, hasher: &mut Sha256) -> Result<()> {
    file.rewind()?;
    let mut buffer = [0u8; 65536];
    while remaining > 0 {
        let n = file.read(&mut buffer[..remaining.min(65536) as usize])?;
        ensure!(n > 0, "history shortened during prefix validation");
        hasher.update(&buffer[..n]);
        remaining -= n as u64;
    }
    Ok(())
}

pub(crate) fn normalize(value: &Value) -> (String, String) {
    let attachment = &value["attachment"];
    if value["customType"] == "sqnic-context" || value["message"]["customType"] == "sqnic-context" {
        return ("sqnic_context".into(), String::new());
    }
    if value["type"] == "attachment"
        && (attachment["command"]
            .as_str()
            .is_some_and(|s| s.starts_with("sqnic-managed:v1:"))
            || attachment["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("SQnic local handoff.")))
    {
        return ("sqnic_context".into(), String::new());
    }
    let outer = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("record");
    let payload = value
        .get("payload")
        .or_else(|| value.get("message"))
        .or_else(|| value.get("item"))
        .unwrap_or(value);
    let inner = payload
        .get("role")
        .or_else(|| payload.get("type"))
        .and_then(Value::as_str);
    let kind = inner
        .filter(|x| *x != outer)
        .map_or_else(|| outer.to_owned(), |x| format!("{outer}/{x}"));
    let mut body = String::new();
    flatten(value, &mut body);
    (kind, body)
}
fn flatten(value: &Value, out: &mut String) {
    match value {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Array(items) => {
            for item in items {
                flatten(item, out)
            }
        }
        Value::Object(fields) => {
            for (key, v) in fields {
                if key == "data"
                    && fields
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|t| matches!(t, "base64" | "image"))
                {
                    continue;
                }
                if matches!(key.as_str(), "encrypted_content" | "signature") {
                    continue;
                }
                out.push_str(key);
                out.push_str(": ");
                flatten(v, out);
            }
        }
        Value::Null => {}
        other => {
            out.push_str(&other.to_string());
            out.push('\n');
        }
    }
}

pub fn refresh_sources(
    store: &mut Store,
    task: &str,
    histories: &[String],
    format: Format,
) -> Result<Vec<Value>> {
    store.repo(task)?;
    let mut sources = store
        .conn
        .prepare("SELECT path,format FROM sources WHERE task=? ORDER BY id")?
        .query_map([task], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Canonicalize supplied paths first, so aliases never hash/import the same source twice.
    for history in histories {
        let path = std::fs::canonicalize(history).with_context(|| {
            format!("resolve history path {history:?}; task registration may already be saved")
        })?;
        ensure!(
            path.is_file(),
            "history must be a regular file: {}",
            path.display()
        );
        let path = path.to_str().context("history path must be UTF-8")?;
        if let Some((_, previous)) = sources.iter_mut().find(|(name, _)| name == path) {
            if format != Format::Auto {
                *previous = serde_json::to_string(&format)?;
            }
        } else {
            sources.push((path.to_owned(), serde_json::to_string(&format)?));
        }
    }
    let mut results = Vec::new();
    for (path, format) in sources {
        match import(store, task, &path, serde_json::from_str(&format)?) {
            Ok(v) => results.push(v),
            Err(e) => {
                return Err(e.context(format!(
                    "sync stopped after {} completed sources; rerun is safe",
                    results.len()
                )));
            }
        }
    }
    Ok(results)
}

fn digest_hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_preserves_tool_arguments_and_results() {
        let (_, text) = normalize(
            &json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"call-a","name":"shell","input":{"command":"cargo test"}},{"type":"tool_result","tool_use_id":"call-a","content":"17 passed"}]}}),
        );
        for expected in ["call-a", "cargo test", "17 passed"] {
            assert!(text.contains(expected));
        }
    }
    #[test]
    fn ciphertext_is_not_indexed() {
        let (_, text) = normalize(&json!({"encrypted_content":"opaque","summary":"visible"}));
        assert!(!text.contains("opaque"));
        assert!(text.contains("visible"));
    }
}
