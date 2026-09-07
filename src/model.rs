use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Auto,
    Jsonl,
    Codex,
    Claude,
    Pi,
    Text,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Goal,
    Constraint,
    Decision,
    Progress,
    Blocker,
    Next,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Constraint => "constraint",
            Self::Decision => "decision",
            Self::Progress => "progress",
            Self::Blocker => "blocker",
            Self::Next => "next",
        }
    }
}
fn budget() -> usize {
    8000
}
fn limit() -> usize {
    20
}
fn key() -> String {
    "main".into()
}

#[derive(Debug, Deserialize, Subcommand)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    /// Register an explicit task and its worktree.
    Create {
        task: String,
        #[arg(long)]
        repo: String,
    },
    /// List known tasks.
    Tasks,
    /// Import a history file; re-run to capture appended JSONL records.
    Import {
        task: String,
        path: String,
        #[arg(long, value_enum, default_value = "auto")]
        #[serde(default)]
        format: Format,
    },
    /// Register an existing project if --repo is supplied, then refresh histories and Git.
    Sync {
        task: String,
        #[arg(long)]
        repo: Option<String>,
        /// Explicit history file; repeat for multiple exports. Paths are relative to the caller.
        #[arg(long = "history")]
        #[serde(default)]
        histories: Vec<String>,
        /// Format for explicitly supplied histories; registered sources retain their format.
        #[arg(long, value_enum, default_value = "auto")]
        #[serde(default)]
        format: Format,
    },
    /// Read a compact brief without running Git or importing data.
    Resume {
        task: String,
        #[arg(long, default_value_t = 8000)]
        #[serde(default = "budget")]
        max_chars: usize,
    },
    /// Search literal terms within one task's indexed records.
    Search {
        task: String,
        query: String,
        #[arg(long, default_value_t = 20)]
        #[serde(default = "limit")]
        limit: usize,
        #[arg(long)]
        #[serde(default)]
        exact: bool,
        /// Only native user requests, excluding records with tool results.
        #[arg(long)]
        #[serde(default)]
        requests_only: bool,
    },
    /// Page through history in ingestion order.
    History {
        task: String,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        after: i64,
        #[arg(long, default_value_t = 20)]
        #[serde(default = "limit")]
        limit: usize,
    },
    /// Read original event text in character slices.
    Read {
        task: String,
        id: i64,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 8000)]
        #[serde(default = "budget")]
        max_chars: usize,
    },
    /// Set one note; --expected-revision prevents overwriting a concurrent update.
    Update {
        task: String,
        #[arg(long, value_enum)]
        kind: Kind,
        #[arg(long, default_value = "main")]
        #[serde(default = "key")]
        key: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        expected_revision: Option<i64>,
        #[arg(long, default_value = "")]
        #[serde(default)]
        scope: String,
    },
    /// Read all note revisions after a revision ID.
    Notes {
        task: String,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        after: i64,
        #[arg(long, default_value_t = 20)]
        #[serde(default = "limit")]
        limit: usize,
    },
    /// Index missing commits from HEAD ancestry and save worktree status.
    GitSync { task: String },
    /// Page through indexed commits.
    Commits {
        task: String,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        #[serde(default = "limit")]
        limit: usize,
    },
    /// Read a commit's evidence, optionally with a bounded local diff.
    Commit {
        task: String,
        hash: String,
        #[arg(long)]
        #[serde(default)]
        diff: bool,
        #[arg(long, default_value_t = 8000)]
        #[serde(default = "budget")]
        max_chars: usize,
    },
    /// Attach an agent-written explanation to a known commit.
    Annotate {
        task: String,
        hash: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        author: String,
    },
    /// Read query-relevant original evidence and scoped state within a whole-response byte budget.
    Evidence {
        task: String,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value = "")]
        #[serde(default)]
        scope: String,
        #[arg(long)]
        as_of: Option<i64>,
        #[arg(long, default_value_t = 8000)]
        #[serde(default = "budget")]
        max_bytes: usize,
    },
    /// Read multiple event IDs or commit:FULL_HASH references in request order.
    ReadMany {
        task: String,
        #[arg(value_delimiter = ',')]
        refs: Vec<String>,
        #[arg(long, default_value_t = 8000)]
        #[serde(default = "budget")]
        max_bytes: usize,
        #[arg(long)]
        as_of: Option<i64>,
    },
    /// Append one exact JSON object with an idempotency key, without rereading external history.
    Capture {
        task: String,
        key: String,
        #[arg(long)]
        record: String,
    },
    /// Attach derived search keywords or a summary to an original event.
    Enrich {
        task: String,
        id: i64,
        #[arg(long)]
        text: String,
        #[arg(long)]
        author: String,
    },
    /// Link original evidence to a commit; this is an attributed assertion, not automatic verification.
    Link {
        task: String,
        id: i64,
        hash: String,
        #[arg(long)]
        relation: String,
        #[arg(long)]
        author: String,
    },
    /// List immutable Git checkpoints for historical evidence queries.
    Checkpoints {
        task: String,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        after: i64,
    },
    /// Read counts and storage metrics.
    Stats { task: String },
    /// Create a validated online backup at a new path.
    Backup { path: std::path::PathBuf },
    /// Restore a backup into a new database path.
    RestoreBackup {
        path: std::path::PathBuf,
        #[arg(long)]
        output: std::path::PathBuf,
    },
    /// Delete one task and its dependent data after exact confirmation.
    DeleteTask {
        task: String,
        #[arg(long)]
        confirm: String,
    },
    /// Install or remove project-local automatic capture adapters.
    Setup {
        #[arg(long, default_value = ".")]
        repo: String,
        #[arg(long, value_enum)]
        harness: Harness,
        #[arg(long)]
        #[serde(default)]
        remove: bool,
    },
    /// Inspect recording, session bindings and capture errors.
    AutoStatus {
        #[arg(long, default_value = ".")]
        repo: String,
    },
    /// Pause automatic capture and exclude current sessions from later backfill.
    Pause {
        #[arg(long, default_value = ".")]
        repo: String,
    },
    /// Enable automatic capture for new sessions after a pause.
    Unpause {
        #[arg(long, default_value = ".")]
        repo: String,
    },
    /// Exclude one native session from future automatic recording and restoration.
    ExcludeSession {
        #[arg(long, default_value = ".")]
        repo: String,
        #[arg(long, value_enum)]
        harness: Harness,
        #[arg(long)]
        session: String,
    },
    /// Resolve the current project task and restore bounded context.
    Restore {
        #[arg(long, default_value = ".")]
        repo: String,
        #[arg(long)]
        task: Option<String>,
        #[arg(long, value_enum)]
        harness: Option<Harness>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value_t = 8000)]
        #[serde(default = "budget")]
        max_bytes: usize,
    },
    /// Receive a native lifecycle event on stdin (normally called by an adapter).
    Hook {
        #[arg(long)]
        repo: String,
        #[arg(long, value_enum)]
        harness: Harness,
    },
    /// Reconcile registered automatic transcripts; without --once, run a leased recorder.
    Record {
        #[arg(long, default_value = ".")]
        repo: String,
        #[arg(long)]
        #[serde(default)]
        once: bool,
    },
    /// Serve the same operations over MCP stdio.
    Serve {
        #[arg(long, value_enum, default_value = "full")]
        #[serde(default)]
        profile: ToolProfile,
    },
}

pub fn clipped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
pub fn validate_limit(n: usize) -> anyhow::Result<()> {
    anyhow::ensure!((1..=100).contains(&n), "limit must be between 1 and 100");
    Ok(())
}
pub fn validate_budget(n: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        (256..=100_000).contains(&n),
        "max_chars must be between 256 and 100000"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ToolProfile {
    #[default]
    Full,
    Handoff,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    Pi,
    Cursor,
}
impl Harness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Cursor => "cursor",
        }
    }
}
