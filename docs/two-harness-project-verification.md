# two-harness project verification

verified locally on 2026-09-07 with Claude Code Haiku (`claude-haiku-4-5-20251001`) and Codex `gpt-5.4-mini`.
The disposable `parcel_cli` source directory was deleted after independent acceptance passed.
The existing fixture retains its Git history and private test logs.
[Machine-readable results](two-harness-project-verification.json) include failed attempts and measurements.

## what the trial proved

Claude created a Python standard-library shipping CLI with its pricing calculation deliberately unfinished.
Codex continued the project using its recorded context and commits.
The first Codex implementation charged one block too few; its own tests accepted the same mistake.
An independent checker rejected it.

The trial exposed a write-oriented restore command that failed inside Codex's sandbox.
SQnic now supplies an absolute command path and supports `--read-only` retrieval without schema migration, capture, or binding writes.
Read-only MCP hides and rejects write tools.
A later Codex attempt successfully read original event `102`, but still misinterpreted the pricing rule.
This was a model error, not missing stored evidence.

Claude then corrected the implementation and tests.
That correction run reached its 240-second limit after making the fix and running tests; it also fell back to native history files after noisy search results.
It is not counted as a clean SQnic-only retrieval success.

Search results had become crowded with copies of earlier tool output.
`search --requests-only` now selects native user-role records and excludes records containing tool results.
Startup request selection uses the same exclusion.
Two subsequent fresh reviews used SQnic to retrieve original event `102`, reviewed commit `02652cd62765377af0e56690fac2da397c041e42`, and confirmed the original requirement without receiving the pricing values in their continuation prompts.
Both models made recoverable CLI syntax mistakes; startup examples now substitute the resolved task ID instead of a literal `TASK` placeholder, covered by an adapter regression test.
That final example substitution was not followed by another paid model run.

| final review | elapsed | original SQnic evidence | expected / actual 101g near | standard-library tests |
| --- | ---: | --- | --- | ---: |
| Claude Haiku | 28.59 s | event 102 | 273 / 273 cents | 28 passed |
| Codex gpt-5.4-mini | 63.88 s | event 102 | 273 / 273 cents | 28 passed |

The independent checker ran 18 valid boundary cases across both zones and six invalid-input cases.
All 24 passed, including exact JSON shape, integer cents, exit codes, stderr/stdout separation, and invocation from another working directory.
Tests ran with `python3 -S`, without third-party packages.
The source directory was deleted only after these checks and both final reviews passed.

## other requested work

- Pi 0.85.0: six callback checks passed through the installed native loader in an isolated installation containing its missing `pi-server` dependency.
  Two live Haiku sessions recovered the exact marker and retry limit through native extension capture and injection.
  The extension was explicitly selected for isolation; global Pi configuration was not changed.
- Cursor: project-local version-1 hooks preserve existing configuration and capture available prompts, responses, and tool observations.
  Native transcript imports remain explicit because no stable identity-header contract was verified.
  The installed Cursor client reported no models for the account, so a live model test remains unverified.
  The contract reference is [Cursor's hooks documentation](https://cursor.com/docs/hooks).
- Storage: online backup, restore to a new destination, and explicit task deletion are implemented and tested, including committed WAL content, isolation, read-only rejection, and dangling symlinks.
- Release preparation: versioned archives, checksums, multi-platform CI, and a gated tag-triggered release workflow are implemented.
  A macOS arm64 archive was built and tested locally.
  No release tag was pushed, no public release was created, and the remote four-platform workflow has not run.

## speed and limits

The recorded multi-harness fixture was measured with capture paused, warm filesystem caches, and 30 release-CLI samples per operation.
These timings include process startup.

| operation | median | p95 |
| --- | ---: | ---: |
| request-only search | 4.305 ms | 4.879 ms |
| read original event | 3.586 ms | 4.515 ms |
| restore snapshot, including Git inspection | 27.721 ms | 32.602 ms |

A separate [10,000-event benchmark](benchmark-final-10k.json) measured selective CLI search at 4.623 ms median and persistent-MCP evidence retrieval at 2.477 ms median.
The measured executable was about 4.33 MB.
The benchmark ran on the same active machine, so tail latency includes unrelated local activity; this is not a cold-cache or cross-device result.
The final task-ID example change followed that benchmark and does not change its query paths.

The model runs took seconds and consumed substantial cached context despite millisecond database reads.
This single project proves a working recovery path, not reliable automatic reasoning across all projects.
The next useful evaluation is a fixed set of independent task oracles that measures first-attempt correctness and retrieval mistakes across harness versions.
Adding embeddings or a graph is not justified by this trial alone.

Local validation: 69 Rust tests, four packaging tests, six native Pi callback checks, formatting, Clippy, and Python lint passed.
