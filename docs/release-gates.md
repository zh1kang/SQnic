# accuracy and latency release gates

this table records the previous local baseline.
The current native and long-history follow-up is described in [remaining verification](remaining-verification.md).
Final measurements and saved release evidence are being refreshed before publication.

## current result

the final local gates pass on macOS arm64.
These are defined synthetic workloads, not proof of perfect behavior on arbitrary projects.
Opus 4.5 replaces Haiku; Codex uses Luna.
No commit, push or release was performed.

| final workload | startup p95 | restore p95 | query restore p95 | batch read p95 | search p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10k records, 32 active tasks, 2,000 tracked files | 91.292 ms | 84.426 ms | 77.902 ms | 24.972 ms | 28.317 ms |
| 100k growing records, one active task | 153.029 ms | 54.028 ms | 69.040 ms | 6.106 ms | 11.939 ms |

each operation uses 50 measured samples after three warmups, with CLI startup included.
The gate requires startup below 250 ms p95 and all four retrieval operations below 100 ms p95.
It rejects partial operation sets and fewer than 50 samples.
Responses must contain the correct task, successful evidence, matching search results and fresh Git checkpoints.
All original JSON records must match the source after ingestion and append reconciliation.
The crash and concurrency tests separately compare complete raw records and check database integrity.
Initial catch-up is reported separately and is not covered by the interactive latency target.
See [all latency runs, including the initial failure](release-gate-latency.json).

## changes caused by failures

the first latency run failed query restore at 112.224 ms p95.
Restore now uses one Git snapshot for branch scope and freshness, avoiding a duplicate branch lookup while preserving detached-HEAD identity.
Timing varied across runs; the reports retain every measured profile rather than claiming the whole difference comes from this change.

both models initially missed a required change buried behind 24 continuation messages.
The brief still contains three request excerpts, but its executable command now expands up to 32 user-request IDs: the first and the latest 31.
Tool-result wrappers are excluded.
`request_refs_omitted` marks histories that exceed this batch.
The instruction explicitly requires every listed ID and explains how to expand budget-limited records and find older requests.
Raw history remains available; the brief does not claim to contain all history.

## live accuracy evidence

the final cohort contains three fresh Opus 4.5 sessions and three fresh Luna sessions.
Every session passed all 27 independent pricing, expiry and deduplication cases.
Every session retrieved the original request and the buried change, cited their source IDs and supplied complete usage telemetry.
No final successful run had a SQnic retrieval error.

| model | successful runs | total run times | uncached input tokens |
| --- | ---: | --- | --- |
| `claude-opus-4-5-20251101` | 3/3 | 35.876, 43.244, 56.140 s | 13,653; 20,430; 20,354 |
| `gpt-5.6-luna` | 3/3 | 35.859, 48.392, 53.634 s | 34,866; 37,894; 39,606 |

these times include model work and tool dispatch, not just SQnic queries.
The handoff was about 9.3 KB, and the batch read returned about 14.5 KB.
This adds context compared with the old three-reference brief; the measured tradeoff is better recovery of omitted changes, not a demonstrated token saving.
The oracle was outside the disposable project and was not accessed by the final model command traces.
The runner replays real SQnic hook output explicitly; this is not a new test of native hook installation or discovery.

all 21 attempts are retained in [the attempt report](release-gate-attempts.json).
Before the batch change, 0/6 passed.
The first batch version passed 4/6; Opus shortened the command in its two failures.
The explicit full-batch instruction then passed all three Opus runs.
The accompanying Luna attempts timed out or reported DNS/connection errors and remain failures.
After DNS resolution recovered, a separate three-run Luna cohort passed on the same product and execution code.
These final cohorts are combined transparently in [the release evidence](release-gate-live.json).
The result does not establish a general success rate or show that infrastructure failures cannot recur.

## enforced checks

normal CI and release verification run six additional CLI/database tests after building the release binary:

- concurrent importers of the same 50k-record source, followed by idempotent retries
- killing an importer during an observed write transaction, then recovering every raw record
- explicit bindings for separate tasks in one repository
- task-scoped commit evidence and rejection of foreign events and commit references
- repeated detached-HEAD restore and rejection after changing detached commits
- same-path, same-size file replacement detection without changing stored originals

Linux CI and release verification run both latency profiles.
The tagged-release workflow also checks the saved live report against the current product and execution-source fingerprint.
It requires complete candidate repeats, distinct fixture identities, the exact configured models, exact boolean success flags, all oracle cases, required evidence and complete telemetry.
Baseline runs cannot satisfy this gate.
Verifier-only functions are excluded from the execution fingerprint so report-validation fixes do not require paid model reruns.
The report retains the original full digest and a verified migration record showing that product and execution code did not change during that verifier correction.

```sh
cargo build --release --locked
SQNIC_TEST_BINARY=target/release/sqnic python3 -m unittest discover -s tests -p '*_test.py'
python3 scripts/benchmark_automatic.py --events 10000 --tasks 32 --worktree-files 2000 --samples 50 --gate --output .artifacts/gate-10k.json
python3 scripts/benchmark_automatic.py --events 100000 --samples 50 --gate --output .artifacts/gate-100k.json
python3 scripts/handoff_acceptance.py --live --buried-update --repeats 3 --output .artifacts/live-gate.json
python3 scripts/handoff_acceptance.py --check-report docs/release-gate-live.json
```

validation: 72 Rust tests, 28 Python tests, formatting, Clippy, Ruff and the extracted macOS arm64 archive smoke test passed.
Remote CI, Windows/Linux target binaries and Cursor live behavior still require external verification.
The latency evidence uses warm local caches and synthetic histories up to 100k records; it does not establish cold-disk, multi-million-record or multi-machine performance.
