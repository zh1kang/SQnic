# native handoff, long history, and release verification

## scope

this follow-up tests automatic harness discovery, omitted requests, branch changes, uncommitted work, larger histories, and packaged binaries.
SQnic keeps its local Rust executable and SQLite design.
No runtime dependencies or model calls were added to the product.

## native handoff

a disposable invoice project used the normal project-local Claude Code and Codex adapters.
The generated Codex hooks were reviewed through its normal trust interface.
Destination prompts did not mention SQnic or contain the missing values.

- Opus 4.5 recorded the original requirements, a tool result, and a later price change.
- a fresh Luna session received startup context and used the supplied SQnic batch command first.
- it recovered the changed rate, environment-check output, and full baseline commit hash.
- Codex then recorded a new token in conversation only.
- a fresh Opus 4.5 session recovered that token through SQnic and checked the implementation.
- a fresh Luna session implemented the project through the normal hook path while preserving an existing uncommitted draft.
- an independent external oracle passed all 14 valid-input and invalid-input cases.
- all six registered native files had zero pending bytes and no capture errors before cleanup.
- recording was paused and the disposable project was deleted after success.

the initial isolated Codex attempt did not receive startup injection and mined SQLite directly.
That attempt is a discovery failure, even though its code was correct.
A separate invalid configuration attempt also remains recorded.
One successful native session made invalid retrieval calls before recovering; the report retains those counts.
These are observed local flows, not a general model success-rate estimate.
See [native results](native-handoff-verification.json).

## correctness changes

older requests can now be enumerated without guessing search terms.
When startup omits requests, it supplies an exclusive `after`/`before` interval and the current branch scope.
`history --requests-only` returns up to 32 original records within 20 KB using the existing batch reader.
It preserves raw fields and flags partial text or exhausted budgets.
The caller must complete those records before advancing `next_after`, then continue until `items` is empty.
Normal `history` retains its existing excerpt interface.

fresh sessions on a different branch now offer existing tasks for explicit selection instead of silently creating a second task.
Capture health counts only the selected branch's registered sources.
Task bindings remain immutable within a native session.
The full hook message is capped at 8,000 UTF-8 bytes to fit the configured delivery limit conservatively.

Git checkpoint freshness describes HEAD, branch, and path status only.
Uncommitted file contents are not stored or fingerprinted.
The response states that boundary explicitly and directs the agent to inspect current files and diffs before editing.
This avoids a full-worktree content scan on every query.
A commit made during a recorder pass can wait for the next pass; restore reports stale metadata in the meantime.

## measurements and release checks

see [release gates](release-gates.md) for final latency and live accuracy results.
The benchmark now checks bounded request-history reads, streams its source-integrity comparison, and reports query memory and initial child-process peak memory where supported.
The Linux cold-cache mode syncs and advises eviction of fixture files before each sample.
It does not flush system caches and does not claim a physically cold disk.

package smoke tests now execute the extracted binary, create a database, import an original Unicode record, and read it back exactly.
The release workflow supports a manual verification run that builds archives without publishing a release.
Only a version-tag push can enter the publishing job.
