# earlier baseline verification

This document records the original v1 checks.
See [the current implementation and measurements](improvements.md) for schema-v3 changes and later verification.

# v1 verification

Verified locally on 2026-09-05 with an Apple M3 Pro, macOS 26.5.2, and Rust/Cargo 1.98.0.
All latency results below use the release executable with warm filesystem caches.
Each query has 50 measured samples; CLI timings include process startup, while MCP timings use one running process.
These measurements exclude model and harness overhead.

## correctness

- `cargo fmt --check`: passed.
- `cargo test --locked`: 16 passed, zero failed (2 normalization tests and 14 process/integration tests).
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: passed.
- `cargo build --release --locked`: passed.
- `python3 -m py_compile scripts/benchmark.py scripts/harness_test.py`: passed.
- `ruff check scripts` and `ruff format --check scripts`: passed.
- bundled skill frontmatter validator: passed.

Coverage includes preservation of native tool records and unknown fields, partial lines, import rollback, changed-prefix rejection, repeated imports, task isolation, note conflicts, concurrent writes, reads during writes, schema migration, Unicode paging, empty repositories, root commit evidence, bounded large diffs, omitted-history reporting, MCP tool calls, protocol errors, and oversized MCP frames.
An independent read-only review found three issues: empty-repository sync, omitted-history reporting, and unbounded diff capture.
All were fixed and regression-tested.
The final review found no remaining material issue in the reviewed persistence, import cursor, Git diff, and MCP boundary paths.

The CI workflow covers Linux, macOS, and Windows, but only macOS was executed in this session.

## latency

| operation | 10k records CLI p50 / p95 ms | 100k records CLI p50 / p95 ms | 100k records MCP p50 ms |
|---|---:|---:|---:|
| tasks | 3.028 / 3.528 | 3.326 / 3.754 | 0.041 |
| resume | 3.683 / 4.135 | 3.276 / 3.718 | 0.116 |
| selective search | 4.001 / 4.501 | 4.906 / 9.938 | 0.650 |
| common-word search | 9.086 / 9.899 | 60.672 / 74.292 | not measured |
| missing-term search | 3.826 / 4.275 | 3.668 / 4.082 | not measured |
| history | 3.676 / 4.202 | 3.430 / 4.033 | 0.158 |
| read | 3.379 / 3.852 | 3.075 / 3.424 | 0.060 |
| notes | 3.104 / 3.759 | 3.239 / 5.681 | 0.111 |
| commits | 3.713 / 4.064 | 3.454 / 5.911 | 0.097 |
| commit metadata | 3.074 / 3.468 | 3.283 / 3.607 | 0.053 |
| commit with diff | 7.562 / 8.000 | 8.279 / 9.034 | not measured |
| stats | 3.049 / 3.223 | 3.135 / 3.574 | 0.053 |
| note update | 3.575 / 3.794 | 3.830 / 5.072 | not measured |

The selective term occurs in 1% of synthetic records.
The common word occurs in nearly all records, requiring much more ranked search work.
The common-word query at 100k exceeds the initial 50 ms target; the selective queries and ordinary reads remain below it.
No candidate cap was introduced to hide this cost or omit matching records.

### measured correction

The first 10k-record baseline computed total raw bytes by reading event data on every `resume` and `stats` call.
Schema v2 maintains counters transactionally instead.
Persistent MCP `resume` p50 fell from 3.745 ms to 0.111 ms on the same workload, about 34 times faster.
CLI `resume` p50 fell from 7.394 ms to 3.683 ms; process startup accounts for much of the remaining time.
The baseline and final runs were sequential on a shared workstation, not an isolated laboratory comparison.

## import and footprint

| measurement | 10k records | 100k records |
|---|---:|---:|
| source history | 9,523,699 bytes | 95,536,698 bytes |
| initial import | 345.807 ms | 5,910.807 ms |
| import throughput | 28,918 records/s | 16,918 records/s |
| import bandwidth | 26.265 MiB/s | 15.414 MiB/s |
| unchanged refresh | 7.782 ms | 52.402 ms |
| append one record, including prefix verification | 7.647 ms | 47.106 ms |
| index 20 commits | 114.352 ms | 152.769 ms |
| logical database size, all tasks | 22,872,064 bytes | 228,249,600 bytes |

The final executable is 3,604,064 bytes, about 3.44 MiB.
On the 10k-record workload, macOS `/usr/bin/time -l` reported peak resident memory of 3,883,008 bytes for `resume` and 5,373,952 bytes for the common-word search.
These are query process measurements, not peak import memory.
Raw text, normalized search text, and FTS indexes make the database larger than the original export; compression is not implemented in v1.

The 100k-record `resume` response was 2,397 bytes compared with a 95.5 MB source file.
This is selective retrieval, not an equivalent-information compression ratio or a measured model-token saving.
A harness must request more detail for facts outside the brief.
Output byte counts and p99 values for each query are retained in the JSON reports.

## live harness handoff

Tested Claude Code 2.1.259 and Codex CLI 0.153.3 using their existing authentication and temporary per-process MCP configuration.
No global configuration was changed.
The fixture contained 120 synthetic history records, two commits, a superseded constraint, and unpredictable verification/handoff markers.
The original history file was deleted after import, so the agents had to use the stored copy.

1. Claude Code used 9 SQnic MCP calls and completed in 22.307 seconds.
2. It recovered the current retry limit of 7, the rejected unlimited-retry approach, the old tool result of 23 passing tests, its random evidence marker, and the latest commit's full hash and diff.
3. It saved a new transfer marker as a progress note.
4. Its actual stream-output history was imported into the same task.
5. Codex used 13 SQnic MCP calls and completed in 41.304 seconds.
6. It recovered the same evidence and Claude's transfer marker without shell calls.
7. Codex's actual JSONL output was also imported successfully.

Final answers were inspected, including the current value of 7 rather than the obsolete 3.
The automated checks inspect final answers, not just tool output containing the expected facts.
[The compact harness report](harness-verification.json) contains final answers, tool names, usage counters, and marker checks.
Raw traces remain in ignored `.artifacts/`.

This is a controlled functionality test, not a benchmark of general reasoning quality or token savings.
Harness usage counters include their own prompts, repeated context, and cache accounting; they cannot be compared directly as SQnic overhead.
The live tests explicitly requested SQnic tools.
Automatic skill discovery, native Cursor ingestion, live Pi usage, inaccessible attachments, and hidden reasoning were not validated.

## reports and reproduction

- [initial 10k baseline](benchmark-baseline.json)
- [final 10k measurements](benchmark-10k.json)
- [100k measurements](benchmark-100k.json)
- [live harness results](harness-verification.json)

Run the scripts documented in the repository README to reproduce with disposable local fixtures.
Model-based harness tests consume account usage; the benchmark script performs no model calls.
