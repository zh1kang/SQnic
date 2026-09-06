# SQnic

Local conversation and Git context that coding agents can share through a CLI or MCP.
One Rust executable, one SQLite database, no account, server, or model API required.

SQnic preserves available source records, keeps task state in small revisioned notes, and returns short briefs with searchable detail.
It does not promise that a short brief contains everything: agents can retrieve the original history and commit evidence when needed.

## install

Build with a current stable Rust toolchain and a C compiler for bundled SQLite.
Git is required for commit operations.

```sh
git clone https://github.com/zh1kang/SQnic.git
cd SQnic
cargo install --path . --locked
sqnic --help
```

The installed executable does not require Rust, Python, a separate SQLite installation, or a network connection.
Build separately for each target platform; prebuilt release downloads are not published yet.

## start and continue a task

```sh
sqnic create auth-fix --repo /absolute/path/to/project
sqnic import auth-fix /absolute/path/to/conversation.jsonl
sqnic git-sync auth-fix
sqnic evidence auth-fix --query "retry constraint" --max-bytes 8000
sqnic read-many auth-fix 42,43 --max-bytes 12000
sqnic search auth-fix "retry constraint"
sqnic read auth-fix 42
sqnic commits auth-fix
sqnic commit auth-fix FULL_COMMIT_HASH --diff
```

Give the next agent the task ID and database location, or point it at a history export and this workflow.
The default database is `$SQNIC_DB`, then `$XDG_DATA_HOME/sqnic/context.sqlite3`, then `~/.local/share/sqnic/context.sqlite3`.
Pass `--db /absolute/path/context.sqlite3` to any command to select a different store.
Task IDs are explicit and tied to a canonical worktree path, so different tasks in the same repository remain separate.

### sync an existing project

```sh
# register the task, import supplied exports, and index existing commits
sqnic sync my-project --repo /absolute/path/to/existing-project \
  --history /absolute/path/to/session.jsonl \
  --history /absolute/path/to/export.md

# later: refresh all registered files and capture a new Git checkpoint
sqnic sync my-project
```

`--repo` creates the task if absent or checks its existing worktree binding; it never retargets a task.
History files are optional: `sync my-project --repo PATH` can start from Git alone.
Repeat `--history` for up to 128 explicit files per call; paths are relative to the caller's working directory.
Canonical path aliases are deduplicated, including already registered sources, so each source is refreshed once per call.
Use `--format text` for supplied exports with an unrecognized extension; the default auto-detects Markdown/text extensions and otherwise reads JSONL.
A registered source keeps its format unless an explicit conflicting format is supplied, which fails.

The full-profile MCP `sqnic_sync` accepts `task`, optional `repo`, optional `histories` (an array of paths), and optional `format`.
The six-tool handoff profile remains unchanged; use the CLI or full profile for sync.
Successful imports and task registration remain saved if a later import or Git sync fails.
Retry the same command to finish; already imported records and commits are not duplicated.
Each successful sync still creates a new dated checkpoint.
A missing registered history file stops sync before Git refresh; restore that file or use `git-sync TASK` to refresh Git separately.

Sync reads supplied history and local Git metadata; it does not fetch remotes, scan private harness stores, copy the whole code tree, or edit `AGENTS.md`.
Existing instruction files remain authoritative and unchanged.
For a large monorepo, register focused task IDs and supply only histories that belong to each task.

### history capture

| source | input | preserved |
|---|---|---|
| Claude Code | transcript or stream-output JSONL | messages, tool calls/results, metadata, parent links, unknown records |
| Codex | rollout or `exec --json` output | response items, available messages, tool activity, metadata |
| Pi | session JSONL | messages, tool results, branch/compaction records and parent links |
| Cursor and other harnesses | text/Markdown export or JSONL | available export content; text has no inferred roles |

```sh
sqnic import auth-fix /path/to/cursor-export.md --format text
sqnic import auth-fix /path/to/pi-session.jsonl --format pi
sqnic sync auth-fix
```

The native format selectors label the source; a lossless JSON object reader preserves fields without depending on a fixed vendor schema.
Known envelopes determine event labels, and nested text, arguments, and outputs are searchable.
Unknown fields remain stored even when their format changes.
Encrypted reasoning and image payloads remain in raw records but are excluded from ordinary text indexing where identified.
Images and attachments are not decoded or downloaded.

Capture is explicit: import supplied paths, then refresh with `import` or `sync`.
There is no background daemon, automatic home-directory scan, or automatic hook installation.
A harness can call these commands from a supported lifecycle hook, but v1 does not ship vendor-specific hooks.
Cursor's internal databases and remote-only histories are not directly read; use an available export.

JSONL records must end with a newline.
An unfinished final record is reported as `pending_bytes` and imported after its newline arrives.
Malformed complete records roll back that file's batch with a line-number error.
Repeated imports are idempotent.
Refreshes stream-hash the old prefix, then parse only new records; they still read old bytes to detect rewrites.
Changed or truncated prefixes fail explicitly; import the new version from a different path.
Text files are immutable snapshots and must be exported to a new path after changes.

### incremental task state

```sh
sqnic update auth-fix --kind goal --text 'repair retry behavior'
sqnic update auth-fix --kind constraint --key retries --text 'at most 7 attempts'
sqnic update auth-fix --kind next --text 'run the integration test against the new limit'
sqnic notes auth-fix
sqnic update auth-fix --kind blocker --key service --text ''
```

Kinds are `goal`, `constraint`, `decision`, `progress`, `blocker`, and `next`.
Use one stable key per fact.
An empty text value clears the current fact; old revisions remain available.
`--expected-revision N` detects concurrent edits; use zero when a key should not exist yet.
Identical updates do not create duplicate revisions.
The tool never calls a model to rewrite the full conversation.

### commits

`git-sync` indexes missing commits reachable from the worktree's HEAD, including merge ancestry, and records a dated branch/status snapshot.
An empty repository is supported.
Previously indexed commits remain available after branch changes; this is a historical index, not a live branch listing.
Git remains the source for patches, which are read only on explicit `commit --diff` requests.
A moved/deleted repository or pruned commit can make its patch unavailable; indexed metadata remains stored.

Automatic summaries use the commit message, parents, author, date, and file statistics.
They do not invent intent or test results.
After reading the diff, an agent can add a semantic explanation:

```sh
sqnic annotate auth-fix FULL_COMMIT_HASH --author claude-code --text 'increases the retry cap to tolerate transient failures; the recorded integration check passed at this revision'
```

The explanation is attributed agent text and stored separately from Git evidence.

## evidence bundles and direct capture

```sh
sqnic evidence auth-fix --query 'retry verification' --scope main --max-bytes 8000
sqnic checkpoints auth-fix
sqnic evidence auth-fix --scope main --as-of 1
sqnic read-many auth-fix 42,43,commit:FULL_COMMIT_HASH --max-bytes 12000
sqnic read-many auth-fix 42@1000 --max-bytes 8000
sqnic capture auth-fix session1-event5 --record '{"session_id":"session1","type":"user","text":"retry policy changed"}'
sqnic update auth-fix --kind constraint --key retries --scope main --text 'at most 7 attempts'
sqnic enrich auth-fix 42 --author codex --text 'exponential backoff retry policy'
sqnic link auth-fix 43 FULL_COMMIT_HASH --relation tests --author codex
```

`evidence` selects current notes and lexical matches, then expands one hop of unique parent/tool relationships.
Unlike strict `search`, it matches any query term and can use attributed enrichment keys.
Original source IDs remain attached; generated text never replaces raw history.
The whole application JSON value is limited by `max_bytes` (512..100000), excluding the CLI newline or MCP transport envelope.
The budget is bytes, not model tokens.
`omitted` and `candidate_window_limited` prevent treating a compact result as exhaustive.
Notes remain atomic; a note too large for the budget is omitted instead of cut into a misleading fragment.

`read-many` accepts 1..32 event IDs, `ID@CHARACTER_OFFSET`, or `commit:FULL_HASH` references.
It preserves input order, including duplicates, and returns per-item status and continuation offsets.
A very small budget may not even fit all status entries; increase it or supply fewer references.

Scopes are explicit: `--scope main` includes global records and records marked `main`; the default includes only global records.
Known `scope`, `gitBranch` and `branch` fields are retained as scopes.
Two scopes do not overwrite each other's notes.
If global and scoped notes conflict, both are returned with their scopes; resolve the conflict explicitly.
`--as-of` uses a checkpoint event watermark, not an inferred wall-clock ordering of messages.
Later events, note revisions and enrichment are excluded.
Unreachable indexed commits are excluded from bundles using the selected Git checkpoint.
Legacy `search`, `notes`, `history` and `commits` remain historical browsing interfaces and may include old or alternative-branch records.

`capture` is an atomic append with a task-scoped idempotency key.
It preserves exact JSON bytes and rejects a retry with different bytes.
It avoids external-prefix reads; ordinary file import retains strict prefix verification.
Native session metadata is extracted from known envelopes while unknown fields remain raw.
Direct capture links require an explicit session ID; imported links also use source-file identity.
A source with no session ID uses its source identity and only unique matching IDs are expanded.

Git sync adds immutable checkpoints and reachable commit sets.
Test links are attributed assertions, not automatic proof of execution at a revision.
Dirty status is recorded, but dirty file contents and live processes are not snapshotted.
`enrich` appends source-linked keywords or summaries, is idempotent for identical content/author, and is never included in historical checkpoint search.
No embedding model, vector service, lossy compression, or automatic model call is required.

## connect a harness

Copy or link [the SQnic skill](skills/sqnic/SKILL.md) into the harness's supported skill directory, or use its content as a project instruction snippet.
The CLI works with any harness that can run commands.
No skill installation is required for direct CLI use.

For MCP clients, launch `sqnic --db /absolute/path/context.sqlite3 serve` over stdio.
The default full profile exposes all 21 tools.
For a smaller handoff catalog, use `serve --profile handoff`: it exposes only `evidence`, `read_many`, `search`, `commit`, `notes` and `update`.
Capture, import and administration remain available through the CLI or full profile.
This reduces tool-schema context; total model token use still depends on the harness and its calls.
Example configuration for clients that accept `mcpServers`:

```json
{
  "mcpServers": {
    "sqnic": {
      "command": "/absolute/path/to/sqnic",
      "args": ["--db", "/absolute/path/context.sqlite3", "serve"]
    }
  }
}
```

Equivalent Codex TOML:

```toml
[mcp_servers.sqnic]
command = "/absolute/path/to/sqnic"
args = ["--db", "/absolute/path/context.sqlite3", "serve"]
```

MCP exposes the same 21 application operations with typed schemas.
It adds no cloud dependency and keeps one process alive to avoid CLI startup overhead.
Existing harness authentication is only needed when a model uses the tools.

## retrieval limits and local data

- `resume --max-chars` bounds the **brief text**, not its JSON envelope, counters, or snapshot.
- `read --offset --max-chars` pages through exact original text by Unicode character; follow `next_offset`.
- `history --after` and `notes --after` page by ID; follow `next_after` until no rows remain.
- `commits --offset` pages by row offset.
- `search` matches all whitespace-separated terms with SQLite tokenization; `--exact` also requires the case-sensitive query substring in the indexed body; it does not accept raw FTS operators or perform semantic/vector search.
- `commit --diff --max-chars` stops reading a large Git patch after a bounded prefix and reports truncation.
- History is not an active-branch replay: supported native links are normalized, ambiguous IDs remain unresolved, and explicit scopes select branch-specific evidence.

All database content stays local unless you copy it or a harness sends retrieved text to its model provider.
Raw history can contain sensitive content; SQnic does not silently redact or alter it.
New Unix data directories use mode `0700`, and new database files use `0600`; existing directory permissions are not changed.
The database is not encrypted at rest.
For backup, stop all SQnic processes before copying the database and any remaining `-wal`/`-shm` files, or use SQLite's online backup API.
Do not put a live WAL database on a network filesystem or use file sync as concurrent multi-device replication.

## verification and measurements

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --release --locked
python3 scripts/benchmark.py --extended --events 10000 --samples 50 --output .artifacts/benchmark.json
python3 scripts/evaluate.py --output .artifacts/evaluation.json
```

The optional live check uses existing Claude Code and Codex accounts and consumes model usage:

```sh
python3 scripts/harness_test.py --output .artifacts/harness-results.json
python3 scripts/harness_test.py --reverse --output .artifacts/harness-reverse.json
```

It creates disposable synthetic history and commits, deletes the source transcript after import, then tests evidence retrieval and a handoff through MCP in the selected direction.
The checker asserts typed current/superseded values, source IDs, reasons, diff values and tool usage.
It does not read personal conversation history or change global harness configuration.

See [schema-v3 implementation and results](docs/improvements.md), [architecture](docs/architecture.md), [verification results](docs/verification.md), and the machine-readable benchmark reports under `docs/`.

Historical expansion must pass the same checkpoint: `read-many TASK 42,commit:FULL_HASH --as-of CHECKPOINT`.
This excludes future links and omits all derived annotations/enrichment, whose creation is not tied to checkpoint event watermarks.
Legacy `read` and `commit` are current historical-index reads, not checkpoint-filtered views.
If `evidence` returns `status: required_state_omitted`, increase the byte budget before using other historical evidence.
The tool returns no lower-priority evidence while a required goal or constraint is missing.

see [the implemented improvements and current measurements](docs/improvements.md) for schema-v3 behavior, the smaller MCP profile, quantitative results and limits.
