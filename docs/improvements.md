# source-preserving handoffs

## implemented

- immutable raw records plus normalized session, parent, tool-call, timestamp and scope metadata.
- conservative identity matching: ambiguous links remain unresolved; related records must have the anchor scope or global scope.
- immutable Git checkpoints, event watermarks and reachable commit sets.
- scoped note revisions and historical evidence retrieval.
- query-aware `evidence`, with atomic required state and a total serialized-byte budget.
- ordered `read-many`, including Unicode paging, per-item errors, commit metadata and matching `--as-of` expansion.
- direct `capture` with task-scoped idempotency keys, avoiding external-prefix reads.
- attributed `enrich` and commit evidence `link`, preserving original records.
- exact-substring filtering for code identifiers through `search --exact`.
- task-selective FTS filtering and an FTS-first query plan for historical evidence.
- optional six-tool MCP handoff profile with matching discovery and dispatch filters.
- stronger CLI/MCP tests, a synthetic retrieval pilot, a controlled search comparison and typed live-handoff checks.

The CLI and MCP share these implementations.
The existing recent `resume` and historical browsing commands remain compatible.
No new Rust dependencies were added.

## invariants

Every source file is imported atomically, including normalized metadata.
An unfinished JSONL line remains pending, while malformed complete input rolls back the batch.
External refresh still verifies the entire consumed prefix.
Direct capture is a separate contract: the producer supplies one record and a stable key; an identical retry is a no-op and changed bytes with that key fail.

Scopes do not overwrite one another's notes.
A `main` record can link to `main` or global records; a global record links only to global records.
This rule is intentionally not symmetric.
Unknown fields remain in the original record and are not promoted to instructions.

Checkpoint queries use the event watermark known at capture, not source timestamps or ingestion-time guesses about real-world order.
Historical expansion must repeat `--as-of` on `read-many`.
Derived annotations and enrichment are omitted from historical expansion because their creation is not attached to checkpoint watermarks.
Legacy `read`, `commit`, `notes`, `history` and `search` remain current historical-index browsing interfaces.

If a required goal or constraint cannot fit, `evidence` returns `required_state_omitted` without lower-priority evidence.
Ordinary excerpts can be omitted; original text can be paged.
A partial page must advance, otherwise the item reports budget exhaustion.
Response budgets include the application JSON but exclude the CLI newline or MCP transport envelope.
They do not claim a model-independent token limit.

Git checkpoints retain observed HEAD, branch and status, not dirty file contents or running processes.
A `tests` link is an attributed assertion that must be inspected, not proof of execution by SQnic.

## measurement design

`benchmark-v3-100k.json` measures a release binary on 100,000 repetitive synthetic records and 50 warm samples per query.
CLI timings include process startup; persistent MCP timings exclude model calls.
The capture/enrich/link loop mostly measures idempotent retries after the first write, not new-write throughput.
A separate `capture_new_record` series uses a unique key for every sample and asserts that each small synthetic record was inserted.

`search-comparison-v3.json` compares the archived pre-change binary against the task-filter implementation using identical synthetic FTS bodies at 1, 10 and 100 tasks.
It uses 100 samples, five warmups and an optimized FTS index after loading.
It isolates search, not import or model quality.

At 10 tasks, common-term search median changed from 41.502 to 6.640 ms.
At 100 tasks, it changed from 42.474 to 2.459 ms.
One-task selective search was 0.554 versus 0.581 ms in that experiment.
The index adds storage and write work; it is not a universal latency improvement.

An initial unconditional task filter slowed one-task selective search, so it was replaced with a selectivity-based choice.
The schema-v3 evidence query also exposed an events-first SQLite plan under a watermark filter.
A direct SQL probe on the same 100,000-record database measured approximately 2,960 ms before and 0.57-0.61 ms warm after forcing FTS first.
That probe excludes response assembly and is not the end-to-end evidence latency.
The interrupted slow benchmark was not counted as a successful final benchmark.

## retrieval pilot

`evaluation-v3.json` records 36 deterministic synthetic episodes across four templates, with four probes at four byte budgets.
Twelve episodes are labeled development and twenty-four are labeled held out; template variants are not independent real-repository tasks.
The gold event IDs stay in the evaluator, outside the memory backend.

The probes cover linked test evidence, an enriched paraphrase, current state and absent evidence.
Other integration tests cover temporal cutoffs, branches, corruption, injection-like text and Unicode paging.
The pilot reports absent questions separately from answer-bearing questions.
It does not measure a model's semantic understanding, real coding continuation or automatic skill discovery.

## verification commands

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --release --locked
uvx ruff check scripts
python3 scripts/evaluate.py --output .artifacts/evaluation.json
python3 scripts/benchmark.py --extended --events 100000 --samples 50 --output .artifacts/benchmark.json
python3 scripts/compare_search.py --before /path/to/old/sqnic --output .artifacts/comparison.json
python3 scripts/harness_test.py --output .artifacts/forward.json
python3 scripts/harness_test.py --reverse --output .artifacts/reverse.json
```

Live harness tests consume the user's existing model usage and use disposable synthetic data.
They assert typed current/superseded values, source IDs, reasons, exact markers, commit hashes, diff values and MCP usage.
They do not alter global configuration or read personal history.
The `--mode legacy` option permits comparison against the pre-change binary with the same answer contract.

## deliberate limits

The recommended local core is implemented.
Embeddings, model-driven graph extraction, a separate vector service and disk compression remain conditional experiments, as the research recommended.
Source-linked enrichment is available without requiring any of them.
No model-dependent quality improvement is assumed from the synthetic retrieval pilot.

Cold-disk behavior, million-event histories, long-running multi-writer load, real-repository continuation and automatic skill discovery need broader evaluation before product-level claims.
Pi and Cursor export compatibility do not mean every proprietary session feature is accessible.
The user retains the choice of model provider; retrieved local data can leave the device when the chosen harness sends it to its model.

Schema migration scans existing history and rebuilds indexes once, atomically.
It preserves originals but can take time and extra temporary disk space on a large database.
Use the normal SQLite backup procedure before upgrading an important store.

## live handoff method and tool catalog

The default full MCP catalog remains compatible with all 21 operations.
`serve --profile handoff` exposes six retrieval/state operations and rejects hidden operations at dispatch.
The serialized `tools/list` result is 8,809 bytes for full and 3,040 bytes for handoff, a 65.5% reduction.
These are JSON bytes, not measured tokenizer counts.

The live experiment compares the archived legacy binary, full-profile evidence bundles and the smaller handoff profile.
It runs Claude Code → Codex and Codex → Claude Code with the same synthetic answer contract.
The first agent saves a random transfer marker; its actual output history is imported before the next agent starts.
The agents must recover current and superseded state, original verification evidence, the rejected approach and its reason, and exact commit/diff facts.

A full-profile reverse run returned all correct facts inside a Markdown JSON fence.
It failed the requested raw-JSON contract and remains a failed strict run in the report.
A separate diagnostic checks the facts after removing that fence; it does not convert the strict failure into a pass.
The checker now records JSON parse failure correctly instead of labeling an empty fallback object as structured output.

Harness defaults were not pinned to one model configuration, and cache/network variation was not controlled.
Input counters include repeated and cached context; they are not a direct billing estimate.
The baseline has only one run per direction, so apparent token or wall-time changes are exploratory.
No universal reduction in tokens, cost or model response time is established.

## final release results

verified locally on macOS with 34 passing Rust tests, formatting, Clippy with warnings denied, a release build, Python lint/format checks and the skill validator.
independent review found no remaining actionable issue in the final reviewed changes.
Linux and Windows CI were configured earlier but were not run locally.

100,000 initial synthetic records, 50 samples per operation, warm filesystem caches:

| operation | CLI p50 / p95 ms | persistent MCP p50 / p95 ms |
|---|---:|---:|
| selective search | 4.508 / 5.384 | 0.736 / 0.823 |
| query-aware evidence | 8.904 / 15.214 | 3.087 / 3.275 |
| three original records | 4.023 / 5.353 | 0.217 / 0.254 |
| new small record capture | not measured | 0.243 / 0.572 |

initial import took 6.630 seconds, about 15,084 records/s.
the executable is 3,862,704 bytes and the final logical database is 239,919,104 bytes.
the richer index adds storage and write work: the original baseline database was 228,249,600 bytes, and its executable was 3,604,064 bytes.
one-task common-word CLI search remains 61.047 ms at the median; this is not a universal search speedup.
all 576 pilot cases passed the budget/state/isolation checks.
at 2,000, 4,000 and 8,000 bytes, each held-out budget recovered 72/72 answer-bearing probe sets and correctly returned no evidence for 24/24 absent probes.
at 1,000 bytes, only 48/72 answer-bearing sets were complete.

all six final smaller-profile handoffs passed, three in each direction, giving 12 checked agent answers.
the table aggregates both directions; legacy has two runs per harness and handoff has six.

| harness | median calls: legacy → handoff | median input tokens including cache: legacy → handoff | median seconds: legacy → handoff |
|---|---:|---:|---:|
| Claude Code | 10.5 → 7 | 56,686 → 39,572 | 26.49 → 18.85 |
| Codex | 11 → 5.5 | 149,889 → 137,424 | 39.34 → 37.48 |

these small sequential samples suggest fewer calls and lower aggregate input with the smaller profile, but do not establish a general causal token or latency improvement.
the full-profile bundle did not consistently lower input tokens, which motivated the smaller catalog.

full results: [latency and footprint](benchmark-v3-100k.json), [retrieval pilot](evaluation-v3.json), [live handoffs including failures](harness-v3-verification.json), and [controlled task-selective search experiment](search-comparison-v3.json).
