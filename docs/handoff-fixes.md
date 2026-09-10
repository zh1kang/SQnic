# handoff fixes and measurements

this is the earlier verification record.
See [current release gates](release-gates.md) for the later Opus/Luna runs and current validation counts.

this pass addresses the five issues from the two-harness project trial.
The baseline is commit `2e9200e`; the candidate is the uncommitted change set measured in the accompanying reports.

## original requests and command errors

the startup brief used to retain only the three latest user requests.
Several short continuation messages could remove the original specification from the brief.
The brief now retains the first recorded request and the two latest requests within its existing byte limit.
It marks omitted requests and truncated excerpts.
A complete, shell-quoted `read-many` command expands those request IDs in one call.
Task ambiguity still requires explicit selection.
Historical requests remain untrusted evidence and can be superseded by later changes.

## requirement correctness

a new opt-in acceptance runner uses a fixed 27-case oracle for pricing, expiry and stable deduplication.
It includes zero values, rounding boundaries, equality, case sensitivity, stale summaries and a later rate change.
The model does not see oracle expectations and cannot change the oracle.
Each harness receives a fresh project; the original transcript is removed before it starts.
The runner executes the actual generated startup context as an explicit replay.
This does not establish native hook discovery or installation.
A passing handoff requires a successful harness run, observed SQnic retrieval and all oracle cases.
A direct-specification control does not require retrieval.

## live acceptance and token use

| run | all 27 cases | observed SQnic reads | retrieval errors | total time | aggregate input | cached input | uncached input |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| baseline, Haiku | failed | 1 | 0 | 53.718 s | 84,736 | 71,828 | 12,908 |
| initial candidate, Haiku | failed | 0 | 0 | 28.451 s | 60,547 | 51,157 | 9,390 |
| final revision, Haiku | passed | 1 | 0 | 15.159 s | 44,092 | 35,363 | 8,729 |
| final revision, fresh repeat | passed | 1 | 0 | 22.498 s | 44,790 | 35,704 | 9,086 |
| direct specification, Haiku | passed | 0 | 0 | 28.074 s | 51,878 | 44,212 | 7,666 |

models were fixed to `claude-haiku-4-5-20251001` and `gpt-5.4-mini`.
Both Codex attempts were blocked by the account error: `The 'gpt-5.4-mini' model is not supported when using Codex with a ChatGPT account.`
No more expensive model was substituted.
The control and confirmation runs used Haiku only after that rejection.

both the baseline and initial candidate failed the independent oracle.
The initial candidate did not retrieve originals and copied stale assistant/tool claims.
The final instruction explicitly requires a complete user-request read before editing and says that assistant summaries and tool output are not user requirements.
Both fresh runs of that revision retrieved the originals and passed every case.
These two successful runs do not establish a general success rate.

final startup context was 6,682 bytes, and each successful model read returned 2,185 bytes.
The model first completed that read at 7.140 and 12.644 seconds from harness launch.
These times include model reasoning and tool dispatch; they are not SQnic query latency.
The full successful runs took 15.159 and 22.498 seconds.

Claude aggregate input is reported input plus cache creation plus cache reads.
Uncached input is reported input plus cache creation.
Codex aggregate input includes cached input, so its uncached count would be aggregate minus cached.
Missing usage is marked unavailable in the current runner.
These counters include repeated harness context across model calls; they are not the token size of the handoff alone.
The direct control used less uncached input than either final handoff run, so this test does not prove a token advantage over a complete, manually supplied specification.
Its purpose is to measure the cost of recovering that specification automatically from stored history.
The live tests used explicit replay, so native automatic discovery remains covered by earlier tests rather than this experiment.

see [the sanitized results, including failures and all oracle cases](handoff-acceptance.json).
Private raw logs remain under `.artifacts/handoff-fixes/`; disposable project directories were removed.

## worktree scaling

the recorder ran the same three Git processes for every recent task.
It now observes the worktree once per reconciliation and shares that observation.
Each task still receives its own checkpoint, and indexing still rechecks HEAD.
Strict consumed-prefix verification remains unchanged.

| workload | baseline median | candidate median | baseline p95 | candidate p95 |
| --- | ---: | ---: | ---: | ---: |
| 32 tasks, 2,000 tracked files, unchanged reconcile | 633.250 ms | 61.242 ms | 797.713 ms | 80.716 ms |
| same worktree, append and reconcile | 872.619 ms | 65.683 ms | 1,155.392 ms | 69.637 ms |
| 100k records, one task, unchanged reconcile | 37.707 ms | 42.629 ms | 43.131 ms | 68.387 ms |
| 100k records, one task, append and reconcile | 61.423 ms | 69.657 ms | 71.092 ms | 83.359 ms |

these are warm CLI measurements on macOS arm64, including process startup.
The many-task comparison uses ten samples per operation; the 100k-record comparison uses twenty.
All original records were recovered in the growing append workload.
Single-task results do not show a speed gain and include a higher candidate tail.
Initial 100k catch-up took 7.008 seconds for baseline and 5.631 seconds for candidate; ingestion code did not change, so these timings do not establish an ingestion improvement.

startup timing was variable.
The first many-task comparison measured 46.997 ms baseline and 80.963 ms candidate medians.
A focused 40-sample repeat measured 89.946 ms and 71.382 ms respectively.
This is not strong evidence of a startup speed change.
The initial candidate output was 9,483 bytes versus 10,013 for baseline.
After the final instruction clarification, a 40-sample check measured 79.414 ms median, 205.341 ms p95 and 9,704 output bytes.
The reconciliation and 100k-record results above precede this final wording-only revision.
See [all paired scaling measurements](handoff-scaling.json), including the less favorable results.

## release coverage

both CI and release checks now validate archive membership and the SHA-256 sidecar, then execute the extracted binary with `--version` and `--help`.
Each release build job installs Python explicitly.
Release verification now includes the same pinned Ruff checks as normal CI.
The macOS arm64 archive smoke test passed locally.
Remote CI and Linux, Windows and macOS x86_64 binaries have not been verified in this pass.
No branch, tag or release was pushed.
Cursor reports `No models available for this account.`, so a live Cursor run remains blocked.

## remaining limits

preserving the initial request does not replace retrieval of omitted middle changes.
A model can still misread correct evidence; the oracle detects known errors rather than proving arbitrary generated code correct.
Full-prefix verification has linear cost in the consumed transcript size when a source grows.
The current measurements justify removing repeated Git scans, not weakening source-integrity checks or adding a new storage layer.

## validation

71 Rust tests and 16 Python tests passed.
Formatting, Clippy and pinned Ruff checks passed.
The release build and extracted macOS arm64 archive smoke test passed.
Independent read-only review covered the recorder, request selection, release changes and acceptance accounting.
Changes are local and uncommitted.

## Luna follow-up

at the user's request, the default Codex test model changed to `gpt-5.6-luna`.
A fresh Codex session completed in 44.539 seconds and passed all 27 independent cases.
The trace contains a successful SQnic read of original events 1, 5 and 7, returning 2,184 bytes of JSON.
Reported input was 115,434 tokens, including 98,048 cached and 17,386 uncached.
This is one run and does not establish a general success rate or a fair model-speed comparison.

Luna combined the read with skill text and Git/file commands.
The final file search had no matches, so the shell group returned exit code 1 after SQnic had already returned the records.
The original parser incorrectly rejected the whole output as a failed read.
The corrected parser extracts structured SQnic record lines and keeps the shell failure count separate.
Empty and error-only payloads still do not count as retrieved evidence.
Regression tests reproduced both cases before the fix and passed afterward.

The saved trace was reanalyzed without another model call; the original report remains intact.
A first-retrieval timestamp cannot be recovered because the original runner did not persist arrival timestamps, so that field remains unavailable.
The earlier Haiku success traces still pass the corrected parser.
The default-model and parser changes passed 12 focused acceptance tests and Ruff.
This resolves the Codex model-availability blocker for the acceptance run; Cursor and remote platform checks remain unchanged.
