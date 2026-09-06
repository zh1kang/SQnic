//! Source-preserving metadata extraction. Unknown native fields remain in raw records.
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

fn string<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| v.get(key).and_then(Value::as_str))
}

pub fn record(conn: &Connection, event: i64, source: Option<i64>, v: &Value) -> Result<()> {
    let payload = v
        .get("payload")
        .or_else(|| v.get("message"))
        .or_else(|| v.get("item"))
        .unwrap_or(v);
    let outer = string(v, &["type"]);
    let scope = string(v, &["scope", "gitBranch", "branch"])
        .or_else(|| string(payload, &["scope", "gitBranch", "branch"]))
        .unwrap_or("");
    let session = string(v, &["sessionId", "session_id", "thread_id"])
        .or_else(|| string(payload, &["sessionId", "session_id", "thread_id"]))
        .or_else(|| {
            if matches!(outer, Some("session_meta" | "session")) {
                string(payload, &["id"])
            } else {
                None
            }
        });
    let prior: Option<(Option<String>, Option<i64>, String)> = if source.is_some() {
        conn.query_row(
            "SELECT session,episode,scope FROM event_meta WHERE source=? ORDER BY event DESC LIMIT 1",
            [source],
            |r| Ok((r.get(0)?, r.get(1)?,r.get(2)?)),
        )
        .optional()?
    } else {
        None
    };
    let session = session.or_else(|| prior.as_ref().and_then(|x| x.0.as_deref()));
    let external = string(v, &["uuid", "id"]).or_else(|| string(payload, &["uuid", "id"]));
    let parent = string(v, &["parentUuid", "parentId", "parent_id"])
        .or_else(|| string(payload, &["parentUuid", "parentId", "parent_id"]));
    let role =
        string(payload, &["role"]).or_else(|| outer.filter(|x| matches!(*x, "user" | "assistant")));
    let parent_episode: Option<i64> = if let (Some(source), Some(parent)) = (source, parent) {
        let parents=conn.prepare("SELECT episode FROM event_meta WHERE source=? AND session IS ? AND external_id=? AND scope IN ('',?) ORDER BY event DESC LIMIT 2")?.query_map(params![source,session,parent,scope],|r|r.get::<_,Option<i64>>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if parents.len() == 1 { parents[0] } else { None }
    } else {
        None
    };
    let episode = if role == Some("user") {
        event
    } else if parent.is_some() {
        parent_episode.unwrap_or(event)
    } else {
        prior
            .as_ref()
            .filter(|x| x.0.as_deref() == session && (x.2.is_empty() || x.2 == scope))
            .and_then(|x| x.1)
            .unwrap_or(event)
    };
    conn.execute("INSERT INTO event_meta(event,source,external_id,parent_external_id,session,source_time,role,episode,scope) VALUES(?,?,?,?,?,?,?,?,?)",params![event,source,external,parent,session,string(v,&["timestamp"]).or_else(||string(payload,&["timestamp"])),role,episode,scope])?;
    tool_refs(conn, event, payload)?;
    Ok(())
}

fn tool_refs(conn: &Connection, event: i64, v: &Value) -> Result<()> {
    let ty = string(v, &["type"]);
    let role = string(v, &["role"]);
    let direction = match ty {
        Some("tool_use" | "toolCall" | "function_call" | "custom_tool_call") => Some("call"),
        Some("tool_result" | "function_call_output" | "custom_tool_call_output") => Some("result"),
        _ if role == Some("toolResult") => Some("result"),
        _ => None,
    };
    if let Some(direction) = direction {
        let keys = if direction == "result" {
            &["tool_use_id", "toolCallId", "call_id", "id"][..]
        } else {
            &["call_id", "id"][..]
        };
        if let Some(id) = string(v, keys) {
            conn.execute(
                "INSERT OR IGNORE INTO tool_refs(event,call_id,direction,name) VALUES(?,?,?,?)",
                params![event, id, direction, string(v, &["name", "toolName"])],
            )?;
        }
    }
    // Traverse known content arrays only: arbitrary tool arguments are data, not nested calls.
    if let Some(items) = v.get("content").and_then(Value::as_array) {
        for item in items {
            tool_refs(conn, event, item)?;
        }
    }
    Ok(())
}

pub fn metadata_at(conn: &Connection, task: &str, event: i64, watermark: i64) -> Result<Value> {
    let row=conn.query_row("SELECT m.source,m.external_id,m.parent_external_id,m.session,m.source_time,m.role,m.episode,m.scope,m.ingested FROM event_meta m JOIN events e ON e.id=m.event WHERE e.task=? AND m.event=?",params![task,event],|r|Ok((r.get::<_,Option<i64>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?))).optional()?;
    let Some((source, external, parent, session, time, role, episode, scope, ingested)) = row
    else {
        return Ok(Value::Null);
    };
    let parent_ids = if (source.is_some() || session.is_some()) && parent.is_some() {
        conn.prepare("SELECT m.event FROM event_meta m JOIN events e ON e.id=m.event WHERE m.source IS ? AND m.session IS ? AND m.external_id=? AND e.task=? AND m.event<=? AND m.scope IN ('',?) ORDER BY m.event LIMIT 2")?.query_map(params![source,session,parent,task,watermark,scope],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let mut refs = Vec::new();
    let mut q = conn.prepare(
        "SELECT call_id,direction,name FROM tool_refs WHERE event=? ORDER BY call_id,direction",
    )?;
    for item in q.query_map([event], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })? {
        let (id, direction, name) = item?;
        let peers = if source.is_some() || session.is_some() {
            conn.prepare("SELECT t.event FROM tool_refs t JOIN event_meta m ON m.event=t.event JOIN events e ON e.id=t.event WHERE t.call_id=? AND t.direction!=? AND m.source IS ? AND m.session IS ? AND e.task=? AND t.event<=? AND m.scope IN ('',?) ORDER BY t.event LIMIT 20")?.query_map(params![id,direction,source,session,task,watermark,scope],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let same:i64=conn.query_row("SELECT count(*) FROM tool_refs t JOIN event_meta m ON m.event=t.event JOIN events e ON e.id=t.event WHERE t.call_id=? AND t.direction=? AND m.source IS ? AND m.session IS ? AND e.task=? AND t.event<=? AND m.scope IN ('',?)",params![id,direction,source,session,task,watermark,scope],|r|r.get(0))?;
        let resolution = if peers.len() == 1 && same == 1 {
            "unique"
        } else if peers.is_empty() {
            "unresolved"
        } else {
            "ambiguous"
        };
        let peers = if resolution == "unique" {
            peers
        } else {
            Vec::new()
        };
        refs.push(json!({"call_id":id,"direction":direction,"name":name,"related_events":peers,"resolution":resolution}));
    }
    Ok(
        json!({"external_id":external,"parent_external_id":parent,"parent_events":parent_ids,"session":session,"source_time":time,"ingested":ingested,"role":role,"episode":episode,"scope":scope,"tools":refs}),
    )
}

pub fn backfill(conn: &Connection) -> Result<()> {
    let mut query = conn.prepare("SELECT id,source,raw FROM events ORDER BY id")?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let event: i64 = row.get(0)?;
        let source: Option<i64> = row.get(1)?;
        let raw: String = row.get(2)?;
        // Legacy text snapshots need not be JSON; their raw bytes are still authoritative.
        let v = serde_json::from_str(&raw).unwrap_or(Value::Null);
        record(conn, event, source, &v)?;
    }
    Ok(())
}
