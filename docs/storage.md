# storage controls

sqnic provides three storage controls:

```text
sqnic --db context.sqlite3 backup context.sqlite3.backup
sqnic --db context.sqlite3 restore-backup context.sqlite3.backup --output restored.sqlite3
sqnic --db context.sqlite3 delete-task task-id --confirm task-id
```

`backup` uses SQLite's online backup API. It can copy a database while the
application is using it, including committed pages that are currently in the
write-ahead log. The destination must be new. The copy is validated with
`integrity_check` and `foreign_key_check` before it is published with an
atomic no-overwrite hard link.

`restore-backup` also requires a new destination and refuses existing files,
directories, and running database paths. It validates both the source and the
restored database. Restore is intentionally a separate operation so it cannot
silently replace the database selected by `--db`.

`delete-task` requires the task ID twice. It runs as one immediate transaction,
marks related automatic sessions excluded, clears their task association, and
deletes task-owned sources, events, notes, metadata, captures, commits,
checkpoints, links, annotations, and enrichments. Task-owned full-text rows are
removed in the transaction. Other tasks and their notes remain available.

Backup and restore preserve raw event content.
Task deletion removes stored task content and registered automatic paths, but retains session identity tombstones to prevent automatic re-import.
All three controls preserve native source files. SQLite's
`secure_delete` setting is not enabled: deleting a row removes it from the
live database and full-text indexes, but deleted bytes can remain in free
pages, WAL files, filesystem snapshots, or backups. A backup is a logical
SQLite copy, not encryption or secure erasure. Protect its path and remove
old backups using the operating system's storage controls when required.
