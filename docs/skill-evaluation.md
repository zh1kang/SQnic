# skill verification

The SQnic skill's description retains its specific SQnic and cross-harness triggers.
Its frontmatter passed the official skill validator.
It has no explicit-only invocation setting or missing local references.

| prompt | expected route | review result |
|---|---|---|
| continue SQnic task auth-fix from its local history | use SQnic and explicit task | supported |
| import this conversation export and prepare a harness handoff | use SQnic capture/evidence workflow | supported |
| recover what was known at checkpoint 4 | evidence and read-many with the same as_of | supported |
| explain SQLite indexes | ordinary explanation, no SQnic activation | description excludes unrelated coding work |
| edit a React button | relevant frontend workflow, no SQnic activation | description excludes unrelated coding work |

These are manual routing checks, not a measured automatic-discovery benchmark.
Live Claude Code/Codex tests explicitly name the MCP tools and verify their outcomes.
They do not prove that either harness discovers an installed skill without a tool-specific prompt.
No global skill directories or harness configuration were changed.
