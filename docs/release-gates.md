# accuracy and latency release gates

## verified local result

the final local gates pass on macOS arm64.
The implementation remains one Rust executable with local SQLite storage and no added runtime dependencies or product model calls.
The full hook message is bounded to 8,000 UTF-8 bytes.
The native hook trial, strict model replay, and query benchmarks are separate checks.
These synthetic workloads do not prove perfect behavior on arbitrary histories.

| workload | startup p95 | restore p95 | query restore p95 | batch read p95 | search p95 | request history p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 10k records, 32 tasks, 2,000 tracked files | 58.113 ms | 50.927 ms | 50.695 ms | 3.261 ms | 4.093 ms | 4.764 ms |
| 100k growing records | 36.799 ms | 29.859 ms | 39.222 ms | 3.205 ms | 7.949 ms | 4.297 ms |
| one million growing records | 42.673 ms | 31.065 ms | 62.633 ms | 3.162 ms | 36.199 ms | 4.517 ms |

each operation uses 50 measured samples after three warmups, including CLI startup and a fresh SQLite connection.
Startup must stay below 250 ms p95; all five retrieval operations must stay below 100 ms p95.
The gate rejects partial operation sets and fewer than 50 samples.
Responses must identify the correct task and contain expected originals, matching search results, and fresh Git metadata.
Every stored JSON record is compared with its source values after ingestion and append reconciliation.
The separate pagination regression compares complete raw records, including Unicode and line endings.
See [all latency reports](release-gate-latency.json), including failures and per-sample measurements.

## memory, storage, and initial import

the one-million-record profile used a 4.37 MB executable.
Measured query process peaks were 4.3 to 8.2 MiB.
The peak reported for child processes through initial ingestion was 9.4 MiB; it is not an isolated importer-only measurement.
Initial background catch-up took 60.43 seconds.
The source occupied 260.7 MB and the database 885.5 MB after reconciliation.
The database contains raw records, normalized evidence, and indexes.
Initial catch-up is separate from the interactive latency target.
Strict append-prefix verification took 145.691 ms p95 at this size.

## live accuracy evidence

the release evidence contains three fresh Opus 4.5 runs and three fresh Luna runs against the same product and execution-source fingerprint.
Each passed all 27 independent pricing, expiry, and deduplication cases.
Each retrieved and cited the original request, an early price update, and a conflicting expiry update on the second request page.
The fixture places these changes behind 48 continuation requests, outside the startup excerpts.
The six accepted runs have complete usage telemetry and no failed SQnic retrieval calls.

| model | passing runs | total run times | reported uncached input tokens |
| --- | ---: | --- | --- |
| `claude-opus-4-5-20251101` | 3/3 | 35.427, 45.681, 52.300 s | 17,968; 17,459; 18,100 |
| `gpt-5.6-luna` | 3/3 | 31.423, 20.145, 21.840 s | 28,322; 26,578; 27,032 |

these times include model work and tool dispatch.
Token accounting differs by provider and is not a billing comparison.
The runner explicitly replays SQnic hook output; native discovery is tested separately.
The source transcript is removed before the model starts, and an external oracle checks the resulting file.
See [source-matched release evidence](release-gate-live.json).

all 66 live replay attempts remain in [the attempt report](release-gate-attempts.json).
Earlier failures exposed missing originals, incorrect precedence, incomplete page traversal, invalid options, and evaluator parsing errors.
The final Opus cohort passed as a group.
One Luna run then mistyped a task ID, was correctly rejected, and recovered; that run remains a strict failure.
A subsequent full three-run Luna cohort passed on the same code and runner.
The release report identifies these separate cohorts explicitly.
This does not establish a general 100% model success rate or eliminate possible model typing errors.

## native handoff and platform checks

a disposable invoice project passed 14 independent cases through normal Claude Code and Codex hooks.
Destination prompts did not mention SQnic or contain the missing values.
The trial recovered a changed requirement, tool output, a commit, and a conversation-only token while preserving an existing uncommitted draft.
The project was deleted after success.
Initial discovery/configuration failures and recovered invalid calls remain in [the native report](native-handoff-verification.json).

local validation passed 78 Rust tests, 29 Python tests, formatting, Clippy, the pinned CI Ruff checks, six Pi callback checks, and an extracted macOS arm64 archive smoke test.
The archive test creates a database, imports an original Unicode record, and reads it back exactly.
CI tests Linux, Windows, macOS arm64, and macOS x86_64.
Release verification checks the saved live report before building all four release targets.
See [current pull request checks and artifacts](https://github.com/zh1kang/SQnic/pull/1/checks) for remote status on the latest commit.
Only a version-tag push can publish; pull-request and manual runs retain artifacts without publication.

## cold-cache evidence and limits

Linux cold-cache tests sync and advise eviction of fixture files before every sample.
They do not flush system caches or establish a controlled physical cold-disk state.
The same 100k workload passed normal CI with cold search at 52.879 ms p95, but two release-verification runs failed at 205.111 and 290.929 ms p95.
In the diagnostic failure, search samples read the same 4,552 input blocks while elapsed time ranged from 66.094 to 468.425 ms.
A passing runner read almost the same amount, with search samples near 48 to 53 ms.
This supports an inference of hosted-runner I/O variability; it does not prove its cause.
No speculative search or index change was made, and the 100 ms gate remains active.
All failures remain recorded; current CI artifacts include each sample's time, major page faults, and input blocks.

Cursor live testing remains blocked because the installed client reports no models available for this account.
Automatic adapter installation supports Unix hosts; Windows supports the CLI.
Git checkpoint freshness covers HEAD, branch, and path status, not uncommitted file contents.
The receiving agent must inspect current files and diffs.
Long histories still require additional reads and tokens; small query latency does not make unlimited context free.

## repeat the local gates

```sh
cargo build --release --locked
SQNIC_TEST_BINARY=target/release/sqnic python3 -m unittest discover -s tests -p '*_test.py'
python3 scripts/benchmark_automatic.py --events 10000 --tasks 32 --worktree-files 2000 --samples 50 --gate --output .artifacts/gate-10k.json
python3 scripts/benchmark_automatic.py --events 100000 --samples 50 --gate --output .artifacts/gate-100k.json
python3 scripts/benchmark_automatic.py --events 1000000 --samples 50 --gate --output .artifacts/gate-1m.json
python3 scripts/handoff_acceptance.py --live --buried-update --repeats 3 --output .artifacts/live-gate.json
python3 scripts/handoff_acceptance.py --check-report docs/release-gate-live.json
```
