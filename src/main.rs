mod adapters;
mod automatic;
mod capture;
mod evidence;
mod git;
mod import;
mod mcp;
mod model;
mod normalize;
mod recorder;
mod storage;
mod store;

use anyhow::Result;
use clap::Parser;
use model::Command;
use serde_json::Value;
use std::{io::Write, path::PathBuf};
use store::Store;

#[derive(Parser)]
#[command(version, about = "Local, portable context for coding agents")]
struct Cli {
    /// Database path. Defaults to SQNIC_DB or ~/.local/share/sqnic/context.sqlite3.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Open an existing database without writes, for sandboxed retrieval.
    #[arg(long, global = true)]
    read_only: bool,
    #[command(subcommand)]
    command: Command,
}
fn main() {
    if let Err(e) = run() {
        eprintln!("sqnic: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.read_only {
        anyhow::ensure!(
            matches!(
                &cli.command,
                Command::Tasks
                    | Command::Resume { .. }
                    | Command::Search { .. }
                    | Command::History { .. }
                    | Command::Read { .. }
                    | Command::ReadMany { .. }
                    | Command::Evidence { .. }
                    | Command::Commit { .. }
                    | Command::Commits { .. }
                    | Command::Notes { .. }
                    | Command::Checkpoints { .. }
                    | Command::Stats { .. }
                    | Command::Restore { .. }
                    | Command::AutoStatus { .. }
                    | Command::Serve { .. }
            ),
            "this command requires a writable database"
        );
    }
    if let Command::RestoreBackup { path, output } = &cli.command {
        return storage::restore_backup(path, output).map(|result| {
            println!("{result}");
        });
    }
    let path = cli.db.map(Ok).unwrap_or_else(store::default_db)?;
    let mut store = if cli.read_only {
        Store::open_read_only(&path)?
    } else {
        Store::open(&path)?
    };
    let db = automatic::database_path(&path)?;
    match &cli.command {
        Command::Setup {
            repo,
            harness,
            remove,
        } => {
            let repo = automatic::root(repo)?;
            if *remove {
                automatic::set_adapter(&store, &repo, *harness, false)?;
            }
            let result = adapters::install(&repo, *harness, &db, *remove)?;
            automatic::set_adapter(&store, &repo, *harness, !remove)?;
            println!("{result}");
            return Ok(());
        }
        Command::Hook { repo, harness } => {
            use std::io::Read;
            let result = (|| -> Result<Value> {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 1024 * 1024, "hook payload exceeds 1 MiB");
                let payload = serde_json::from_slice(&bytes)?;
                let repo = automatic::root(repo)?;
                automatic::hook(&mut store, &repo, *harness, payload, &db)
            })();
            match result {
                Ok(v) => println!("{v}"),
                Err(e) => println!(
                    "{}",
                    serde_json::json!({"systemMessage":format!("SQnic capture/restore incomplete: {e:#}. Run sqnic auto-status for details.")})
                ),
            }
            return Ok(());
        }
        Command::Record { repo, once } => {
            let repo = automatic::root(repo)?;
            println!("{}", recorder::run(&mut store, &repo, *once)?);
            return Ok(());
        }
        _ => {}
    }
    if let Command::Serve { profile } = cli.command {
        return mcp::serve(&mut store, profile);
    }
    let result = execute(&mut store, cli.command)?;
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &result)?;
    writeln!(out)?;
    Ok(())
}
fn execute(store: &mut Store, command: Command) -> Result<Value> {
    match command {
        Command::Create { task, repo } => store.create(&task, &repo),
        Command::Tasks => store.tasks(),
        Command::Import { task, path, format } => import::import(store, &task, &path, format),
        Command::Sync {
            task,
            repo,
            histories,
            format,
        } => {
            anyhow::ensure!(
                histories.len() <= 128,
                "supply at most 128 history files per sync"
            );
            anyhow::ensure!(
                !histories.is_empty() || format == model::Format::Auto,
                "--format requires --history"
            );
            if let Some(repo) = repo {
                store.create(&task, &repo)?;
            }
            let sources = import::refresh_sources(store, &task, &histories, format)?;
            let git = git::sync(store, &task).map_err(|e| e.context(format!("sync Git failed after {} completed sources; registration and imports remain saved; rerun is safe", sources.len())))?;
            Ok(serde_json::json!({"sources":sources,"git":git}))
        }
        Command::Resume { task, max_chars } => store.resume(&task, max_chars),
        Command::Search {
            task,
            query,
            limit,
            exact,
            requests_only,
        } => store.search(&task, &query, limit, exact, requests_only),
        Command::History {
            task,
            after,
            limit,
            requests_only,
            scope,
            before,
        } => store.history(&task, after, limit, requests_only, scope.as_deref(), before),
        Command::Read {
            task,
            id,
            offset,
            max_chars,
        } => store.read(&task, id, offset, max_chars),
        Command::Update {
            task,
            kind,
            key,
            text,
            expected_revision,
            scope,
        } => store.update(&task, kind, &key, &text, expected_revision, &scope),
        Command::Notes { task, after, limit } => store.notes(&task, after, limit),
        Command::GitSync { task } => git::sync(store, &task),
        Command::Commits {
            task,
            offset,
            limit,
        } => git::commits(store, &task, offset, limit),
        Command::Commit {
            task,
            hash,
            diff,
            max_chars,
        } => git::commit(store, &task, &hash, diff, max_chars),
        Command::Annotate {
            task,
            hash,
            text,
            author,
        } => git::annotate(store, &task, &hash, &text, &author),
        Command::Evidence {
            task,
            query,
            scope,
            as_of,
            max_bytes,
        } => evidence::bundle(store, &task, query.as_deref(), &scope, as_of, max_bytes),
        Command::ReadMany {
            task,
            refs,
            max_bytes,
            as_of,
        } => evidence::read_many(store, &task, &refs, max_bytes, as_of),
        Command::Capture { task, key, record } => capture::append(store, &task, &key, &record),
        Command::Enrich {
            task,
            id,
            text,
            author,
        } => evidence::enrich(store, &task, id, &text, &author),
        Command::Link {
            task,
            id,
            hash,
            relation,
            author,
        } => evidence::link(store, &task, id, &hash, &relation, &author),
        Command::Checkpoints { task, after } => evidence::checkpoints(store, &task, after),
        Command::Stats { task } => store.stats(&task),
        Command::Backup { path } => storage::backup(&store.conn, &path),
        Command::RestoreBackup { path, output } => storage::restore_backup(&path, &output),
        Command::DeleteTask { task, confirm } => {
            storage::delete_task(&mut store.conn, &task, &confirm)
        }
        Command::Restore {
            repo,
            task,
            harness,
            session,
            query,
            max_bytes,
        } => {
            let repo = automatic::root(&repo)?;
            let read_only = store.conn.is_readonly("main")?;
            if session.is_some() && !read_only {
                automatic::restore(
                    store,
                    &repo,
                    task.as_deref(),
                    harness,
                    session.as_deref(),
                    query.as_deref(),
                    max_bytes,
                )?;
            }
            let reconciliation = if read_only {
                serde_json::json!({})
            } else {
                recorder::reconcile(store, &repo, true)?
            };
            let mut restored = automatic::restore(
                store,
                &repo,
                task.as_deref(),
                harness,
                session.as_deref(),
                query.as_deref(),
                max_bytes,
            )?;
            if reconciliation["reconciliation_busy"] == true && restored["capture"].is_object() {
                restored["capture"]["freshness"] =
                    serde_json::json!("reconciliation busy; retry restore");
            }
            Ok(restored)
        }
        Command::AutoStatus { repo } => automatic::status(store, &automatic::root(&repo)?),
        Command::Pause { repo } => automatic::pause(store, &automatic::root(&repo)?),
        Command::Unpause { repo } => automatic::enable(store, &automatic::root(&repo)?),
        Command::ExcludeSession {
            repo,
            harness,
            session,
        } => automatic::exclude(store, &automatic::root(&repo)?, harness, &session),
        Command::Serve { .. }
        | Command::Setup { .. }
        | Command::Hook { .. }
        | Command::Record { .. } => anyhow::bail!("process lifecycle commands cannot be nested"),
    }
}
