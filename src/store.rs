use crate::model::{Kind, clipped, validate_budget, validate_limit};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Store {
    pub conn: Connection,
}
impl Store {
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("open existing context database read-only")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            version == 4,
            "read-only retrieval requires schema 4; migrate with a writable SQnic command first"
        );
        Ok(Self { conn })
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
            && !parent.exists()
        {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent)?;
        }
        if !path.exists() {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mut conn = Connection::open(path).context("open context database")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            version <= 4,
            "database schema {version} is newer than this binary"
        );
        if version < 4 {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            ensure!(version <= 4, "database schema changed; upgrade this binary");
            if version == 0 {
                tx.execute_batch(include_str!("schema.sql"))?;
            }
            if version < 2 {
                tx.execute_batch(include_str!("migration_v2.sql"))?;
            }
            if version < 3 {
                tx.execute_batch(include_str!("migration_v3.sql"))?;
                crate::normalize::backfill(&tx)?;
            }
            if version < 4 {
                tx.execute_batch(include_str!("migration_v4.sql"))?;
            }
            tx.commit()?;
        }
        Ok(Self { conn })
    }
    pub fn repo(&self, task: &str) -> Result<String> {
        self.conn
            .query_row("SELECT repo FROM tasks WHERE id=?", [task], |r| r.get(0))
            .optional()?
            .with_context(|| format!("unknown task {task:?}; use create first"))
    }
    pub fn create(&self, task: &str, repo: &str) -> Result<Value> {
        ensure!(
            !task.is_empty() && task.len() <= 128 && !task.chars().any(char::is_control),
            "task ID must contain 1..128 bytes and no control characters"
        );
        let path = fs::canonicalize(repo).context("resolve worktree path")?;
        ensure!(path.is_dir(), "repo must be a directory");
        let path = path.to_str().context("worktree path must be UTF-8")?;
        self.conn.execute(
            "INSERT INTO tasks(id,repo) VALUES(?,?) ON CONFLICT(id) DO NOTHING",
            params![task, path],
        )?;
        ensure!(
            self.repo(task)? == path,
            "task already belongs to a different worktree"
        );
        Ok(json!({"task":task,"repo":path}))
    }
    pub fn tasks(&self) -> Result<Value> {
        let mut q = self
            .conn
            .prepare("SELECT id,repo,created FROM tasks ORDER BY id")?;
        let rows=q.query_map([],|r|Ok(json!({"task":r.get::<_,String>(0)?,"repo":r.get::<_,String>(1)?,"created":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!(rows))
    }
    pub fn update(
        &mut self,
        task: &str,
        kind: Kind,
        key: &str,
        text: &str,
        expected: Option<i64>,
        scope: &str,
    ) -> Result<Value> {
        self.repo(task)?;
        ensure!(scope.len() <= 256, "scope is too long");
        ensure!(
            !key.is_empty() && key.len() <= 128,
            "key must contain 1..128 bytes"
        );
        ensure!(
            text.len() <= 32_000,
            "note must be at most 32000 bytes; store long history with import"
        );
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current:Option<(i64,String)>=tx.query_row("SELECT id,text FROM notes WHERE task=? AND kind=? AND key=? AND scope=? ORDER BY id DESC LIMIT 1",params![task,kind.as_str(),key,scope],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let revision = current.as_ref().map_or(0, |x| x.0);
        if let Some(e) = expected {
            ensure!(
                e == revision,
                "revision conflict: expected {e}, current {revision}"
            );
        }
        if current.as_ref().is_some_and(|x| x.1 == text) {
            return Ok(json!({"revision":revision,"changed":false}));
        }
        tx.execute(
            "INSERT INTO notes(task,kind,key,text,previous,scope) VALUES(?,?,?,?,?,?)",
            params![task, kind.as_str(), key, text, current.map(|x| x.0), scope],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO events(task,kind,body,raw) VALUES(?,?,?,?)",
            params![
                task,
                "note",
                format!("{} {key}: {text}", kind.as_str()),
                json!({"revision":id,"kind":kind,"key":key,"text":text,"scope":scope}).to_string()
            ],
        )?;
        let event = tx.last_insert_rowid();
        tx.execute("UPDATE notes SET event=? WHERE id=?", params![event, id])?;
        crate::normalize::record(&tx, event, None, &json!({"scope":scope}))?;
        tx.commit()?;
        Ok(json!({"revision":id,"event":event,"scope":scope,"changed":true}))
    }
    pub fn history(
        &self,
        task: &str,
        after: i64,
        limit: usize,
        requests_only: bool,
        scope: Option<&str>,
        before: Option<i64>,
    ) -> Result<Value> {
        self.repo(task)?;
        validate_limit(limit)?;
        ensure!(
            !requests_only || limit <= 32,
            "request history accepts at most 32 originals per page"
        );
        ensure!(
            after >= 0 && before.is_none_or(|id| id >= 0),
            "history cursors must be non-negative"
        );
        let mut q=self.conn.prepare("SELECT e.id,e.kind,substr(e.body,1,600),s.path,e.line,length(e.raw) FROM events e LEFT JOIN sources s ON s.id=e.source WHERE e.task=? AND e.id>? AND e.id<? AND (? IS NULL OR EXISTS(SELECT 1 FROM event_meta m WHERE m.event=e.id AND m.scope IN ('',?))) AND (?=0 OR (EXISTS(SELECT 1 FROM event_meta m WHERE m.event=e.id AND m.role='user') AND NOT EXISTS(SELECT 1 FROM tool_refs t WHERE t.event=e.id AND t.direction='result'))) ORDER BY e.id LIMIT ?")?;
        let rows = q
            .query_map(
                params![
                    task,
                    after,
                    before.unwrap_or(i64::MAX),
                    scope,
                    scope,
                    requests_only,
                    limit as i64
                ],
                event_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if requests_only {
            let refs: Vec<String> = rows.iter().map(|row| row["id"].to_string()).collect();
            let mut out = if refs.is_empty() {
                json!({"historical_data":true,"omitted":false,"items":[]})
            } else {
                crate::evidence::read_many(self, task, &refs, 19800, None)?
            };
            out["next_after"] = rows.last().map_or(Value::Null, |row| row["id"].clone());
            out["page_complete"] = json!(
                out["items"]
                    .as_array()
                    .context("items")?
                    .iter()
                    .all(|item| item["status"] == "ok" && item["data"]["next_offset"].is_null())
            );
            return Ok(out);
        }
        Ok(json!({"next_after":rows.last().and_then(|r|r.get("id")),"events":rows}))
    }
    pub fn read(&self, task: &str, id: i64, offset: usize, max: usize) -> Result<Value> {
        self.read_at(task, id, offset, max, None)
    }
    pub fn read_at(
        &self,
        task: &str,
        id: i64,
        offset: usize,
        max: usize,
        watermark: Option<i64>,
    ) -> Result<Value> {
        self.repo(task)?;
        validate_budget(max)?;
        ensure!(offset <= i32::MAX as usize, "offset is too large");
        let row=self.conn.query_row("SELECT substr(raw,?,?),length(raw),kind,source,line FROM events WHERE task=? AND id=? AND id<=?",params![offset as i64+1,max as i64,task,id,watermark.unwrap_or(i64::MAX)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)? as usize,r.get::<_,String>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,Option<i64>>(4)?))).optional()?.context("event not found in this task")?;
        let next = offset.saturating_add(row.0.chars().count());
        Ok(
            json!({"id":id,"kind":row.2,"source":row.3,"line":row.4,"text":row.0,"total_chars":row.1,"next_offset":if next<row.1 {Some(next)}else{None},"historical_data":true,"metadata":crate::normalize::metadata_at(&self.conn,task,id,watermark.unwrap_or(i64::MAX))?,"derived":if watermark.is_none(){crate::evidence::enrichments(self,task,id)?}else{json!([])}}),
        )
    }
    pub fn search(
        &self,
        task: &str,
        query: &str,
        limit: usize,
        exact: bool,
        requests_only: bool,
    ) -> Result<Value> {
        self.repo(task)?;
        validate_limit(limit)?;
        ensure!(query.len() <= 4000, "query too long");
        let terms: Vec<_> = query
            .split_whitespace()
            .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
            .collect();
        ensure!(!terms.is_empty(), "query cannot be empty");
        let mut q=self.conn.prepare("SELECT e.id,e.kind,snippet(event_fts,0,'[',']',' … ',48),s.path,e.line,length(e.raw) FROM event_fts JOIN events e ON e.id=event_fts.rowid LEFT JOIN sources s ON s.id=e.source WHERE event_fts MATCH ? AND e.task=? AND (?=0 OR instr(e.body,?)>0) AND (?=0 OR (EXISTS(SELECT 1 FROM event_meta m WHERE m.event=e.id AND m.role='user') AND NOT EXISTS(SELECT 1 FROM tool_refs t WHERE t.event=e.id AND t.direction='result'))) ORDER BY rank LIMIT ?")?;
        let rows = q
            .query_map(
                params![
                    scoped_match(&self.conn, task, &terms.join(" AND "))?,
                    task,
                    exact,
                    query,
                    requests_only,
                    limit as i64
                ],
                event_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"matches":rows,"query":query,"exact":exact,"requests_only":requests_only}))
    }
    pub fn notes(&self, task: &str, after: i64, limit: usize) -> Result<Value> {
        self.repo(task)?;
        validate_limit(limit)?;
        let mut q=self.conn.prepare("SELECT id,kind,key,text,previous,created,scope,event FROM notes WHERE task=? AND id>? ORDER BY id LIMIT ?")?;
        let rows=q.query_map(params![task,after,limit as i64],|r|Ok(json!({"revision":r.get::<_,i64>(0)?,"kind":r.get::<_,String>(1)?,"key":r.get::<_,String>(2)?,"text":r.get::<_,String>(3)?,"previous":r.get::<_,Option<i64>>(4)?,"created":r.get::<_,String>(5)?,"scope":r.get::<_,String>(6)?,"event":r.get::<_,Option<i64>>(7)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"notes":rows,"next_after":rows.last().and_then(|x|x.get("revision"))}))
    }
    pub fn resume(&self, task: &str, max: usize) -> Result<Value> {
        self.repo(task)?;
        validate_budget(max)?;
        let mut text = String::from(
            "historical context; verify current code. empty notes clear earlier values.\n",
        );
        let mut q=self.conn.prepare("SELECT n.id,n.kind,n.key,n.text FROM notes n WHERE n.task=? AND n.scope='' AND n.id=(SELECT max(m.id) FROM notes m WHERE m.task=n.task AND m.kind=n.kind AND m.key=n.key AND m.scope=n.scope) ORDER BY CASE n.kind WHEN 'goal' THEN 0 WHEN 'constraint' THEN 1 WHEN 'next' THEN 2 WHEN 'blocker' THEN 3 ELSE 4 END,n.id DESC")?;
        let rows = q.query_map([task], |r| {
            Ok(format!(
                "note {} [{}:{}] {}\n",
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?
            ))
        })?;
        let mut omitted = false;
        for row in rows {
            append_budget(&mut text, &row?, max.saturating_mul(2) / 3, &mut omitted);
        }
        let mut q=self.conn.prepare("SELECT id,kind,substr(body,1,500) FROM events WHERE task=? AND kind!='note' ORDER BY id DESC LIMIT 13")?;
        for (index, row) in q
            .query_map([task], |r| {
                Ok(format!(
                    "event {} [{}] {}\n",
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?
                ))
            })?
            .enumerate()
        {
            if index == 12 {
                omitted = true;
                break;
            }
            append_budget(&mut text, &row?, max, &mut omitted);
        }
        let snapshot: Option<String> =
            self.conn
                .query_row("SELECT snapshot FROM tasks WHERE id=?", [task], |r| {
                    r.get(0)
                })?;
        Ok(
            json!({"task":task,"brief":text,"brief_chars":text.chars().count(),"estimated_tokens":text.len().div_ceil(4),"token_estimate_method":"UTF-8 bytes / 4; not a hard token bound","excerpt_only":true,"omitted":omitted,"snapshot":snapshot.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,"coverage":self.stats(task)?,"next":"search for details; history/read for originals; commits/commit for Git evidence"}),
        )
    }
    pub fn stats(&self, task: &str) -> Result<Value> {
        self.repo(task)?;
        let (events, bytes): (i64, i64) = self.conn.query_row(
            "SELECT event_count,raw_bytes FROM tasks WHERE id=?",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let sources: i64 =
            self.conn
                .query_row("SELECT count(*) FROM sources WHERE task=?", [task], |r| {
                    r.get(0)
                })?;
        let commits: i64 =
            self.conn
                .query_row("SELECT count(*) FROM commits WHERE task=?", [task], |r| {
                    r.get(0)
                })?;
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(
            json!({"events":events,"raw_bytes":bytes,"sources":sources,"commits":commits,"database_logical_bytes_all_tasks":pages*size}),
        )
    }
}
fn append_budget(out: &mut String, text: &str, max: usize, omitted: &mut bool) {
    if out.chars().count() + text.chars().count() <= max {
        out.push_str(text);
    } else {
        *omitted = true;
    }
}
fn event_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"id":r.get::<_,i64>(0)?,"kind":r.get::<_,String>(1)?,"excerpt":clipped(&r.get::<_,String>(2)?,1000),"path":r.get::<_,Option<String>>(3)?,"line":r.get::<_,Option<i64>>(4)?,"raw_chars":r.get::<_,i64>(5)?}),
    )
}
pub fn default_db() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SQNIC_DB") {
        return Ok(path.into());
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .context("set SQNIC_DB or pass --db")?;
    Ok(base.join("sqnic/context.sqlite3"))
}

/// Push task selection into FTS while retaining the exact SQL task predicate.
pub(crate) fn scoped_match(conn: &Connection, task: &str, expression: &str) -> Result<String> {
    let selective:bool=conn.query_row("SELECT event_count*4 < (SELECT coalesce(sum(event_count),0) FROM tasks) FROM tasks WHERE id=?",[task],|r|r.get(0))?;
    Ok(if selective && task.chars().any(char::is_alphanumeric) {
        format!(
            "body : ({expression}) AND task : \"{}\"",
            task.replace('"', "\"\"")
        )
    } else {
        format!("body : ({expression})")
    })
}
