# automatic handoff verification

This is the initial implementation snapshot.
See the [later project trial](two-harness-project-verification.md) for completed Pi live checks, Cursor adapter support, storage controls, and sandbox retrieval fixes.

verified on 2026-09-07 on macOS arm64.
the earlier implementation was committed as `8cf6d29`; the automatic handoff changes are separate working-tree changes.

## correctness

- `cargo test --locked`: 58 passing tests, including 14 automatic lifecycle and 7 adapter tests.
- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `uv tool run ruff check scripts/benchmark_automatic.py`: passed.
- `node scripts/pi_adapter_test.mjs`: six lifecycle checks passed with no model calls.
- independent review found no remaining high-severity defects after the privacy, concurrency, Git freshness and descriptor-validation fixes.

the regression tests cover stable task binding, ambiguity, explicit selection with immediate import, quoted paths, symlink refusal, configuration preservation, paused-session backfill, adapter removal, branch changes, external transcript rejection, delayed/partial/rewritten files, recorder crash recovery, shared reconciliation locks, original provenance, context budgets and Git commits made without new chat text.
these tests cover the stated contracts; they do not prove that every future harness format or failure mode is supported.

## real harness handoff

a real Claude Code session on `claude-haiku-4-5-20251001` recorded a new fixture marker and retry limit.
a fresh Codex session on `gpt-5.4-mini` recovered both exact values through startup context.
the destination prompt did not mention SQnic or supply the answers.
the source used project settings and `dontAsk` with tools disabled; the destination used a read-only sandbox.
Codex's generated hooks had been approved through the normal hook trust interface.
no trust bypass or global model-setting change was used.

the low-cost Claude run reported approximately $0.0181; no Codex dollar estimate is inferred from subscription usage.
Codex reported 25,258 input tokens, including 5,376 cached tokens, and 82 output tokens.
these are whole-harness counts, including its own instructions and context, not the size of the SQnic brief.
see [the machine-readable live result](automatic-harness-verification.json).

an earlier Claude run received the injected context but did not recover the facts correctly.
the restore path was changed to reserve space for original user requests before lower-priority history, and the subsequent real test passed.
this distinction matters: successful hook execution alone is not proof of useful handoff.

Pi's TypeScript extension callbacks passed startup retention, hidden injection, no repeated injection, process failure, malformed output and warning-delivery checks.
the installed Pi 0.85.0 native loader could not resolve `@earendil-works/pi-server`.
full Pi agent behavior remains unverified; the installed global package was not changed to hide that failure.

## latency and storage

all values below are milliseconds, shown as median / p95.
each operation has three warmups and 50 measured samples.
the workload uses one disposable worktree, one synthetic Claude-shaped JSONL transcript, and an artificial worker lease so background activity cannot alter the fixture during measurement.
CLI process startup, SQLite access and relevant Git checks are included.
append reconciliation includes strict prefix verification but excludes the fixture's file-append time.
the host was not isolated from other local work, so tail values should not be treated as service-level guarantees.

| operation | 10,000 records | 100,000 records |
|---|---:|---:|
| startup hook | 104.5 / 150.5 | 114.8 / 262.5 |
| prompt hook | 35.2 / 39.0 | 35.1 / 48.9 |
| restore | 82.5 / 95.8 | 75.3 / 153.5 |
| restore with query | 103.1 / 194.7 | 73.7 / 107.1 |
| capture status | 22.3 / 32.2 | 13.8 / 17.8 |
| unchanged reconciliation | 120.7 / 343.9 | 54.7 / 62.1 |
| append reconciliation | 73.9 / 105.9 | 90.8 / 121.8 |

in the 100,000-record fixture, the roughly 26 MB initial transcript exceeded the foreground scan cap.
startup returned in about 83 ms with pending coverage; the first background import took about 7.8 seconds.
the SQLite database occupied about 89 MB after indexing, including originals and indexes.
a large first import is distinct from a warm restore or an incremental append.
these results support interactive local handoff, but are not a controlled performance comparison with another repository.

reproduce with:

```sh
cargo build --release --locked
python3 scripts/benchmark_automatic.py --events 10000 --output /tmp/automatic-10k.json
python3 scripts/benchmark_automatic.py --events 100000 --output /tmp/automatic-100k.json
node scripts/pi_adapter_test.mjs
# optional: use the installed Pi loader, when its dependencies are complete
node scripts/pi_adapter_test.mjs /absolute/path/to/pi/dist/core/extensions/loader.js
```

the [10k](benchmark-automatic-10k.json) and [100k](benchmark-automatic-100k.json) reports record the tested executable hash and exact sizes.
benchmarks were measured before final installer type cleanup and hook command-quoting cleanup; the storage and query algorithms did not change afterward.

## remaining limits

- automatic installation supports Claude Code, Codex and Pi on Unix; Cursor requires explicit export/import.
- a native adapter must run before a session is discoverable; there is no scan of all older personal conversations.
- startup briefs have a byte budget and can omit detail; the agent must retrieve originals when needed.
- background import still verifies the complete consumed prefix of a changed file.
- each pass checks up to 256 files and up to 64 tasks with hook activity in the previous two minutes.
- old task checkpoints can be stale; restore reports this and `git-sync TASK` provides explicit reconciliation.
- pausing or removing adapters retains old stored data; resume recording in a fresh session.
- filesystem identity and metadata checks are not a sandbox against hostile same-user processes.
