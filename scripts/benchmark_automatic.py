"""Measure automatic handoff on disposable native transcript fixtures, without a model."""

from __future__ import annotations

import argparse
import hashlib
import json
import itertools
import os
import platform
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path
from contextlib import closing

from benchmark import summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/sqnic"))
    parser.add_argument("--events", type=int, default=10000)
    parser.add_argument("--samples", type=int, default=50)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--operations", nargs="+", choices=["startup_hook","prompt_hook","restore","restore_query","read_many","search","request_history","status","unchanged_reconcile","append_and_reconcile"])
    parser.add_argument("--tasks", type=int, default=1)
    parser.add_argument("--worktree-files", type=int, default=0)
    parser.add_argument("--gate", action="store_true", help="require 50 samples, retrieval p95 <100ms and startup p95 <250ms")
    parser.add_argument("--cold-cache", action="store_true", help="advise Linux to evict fixture file caches before each sample; does not flush system caches")
    args = parser.parse_args()
    if args.cold_cache and not hasattr(os, "posix_fadvise"):
        parser.error("--cold-cache requires posix_fadvise (Linux)")
    if args.events < 20 or args.samples < 2:
        parser.error("use at least 20 events and 2 samples")
    if not 1 <= args.tasks <= 64 or not 0 <= args.worktree_files <= 100000:
        parser.error("use 1..64 tasks and 0..100000 worktree files")
    required = {"startup_hook", "restore", "restore_query", "read_many", "search", "request_history"}
    if args.gate and (args.samples < 50 or (args.operations and not required.issubset(args.operations))):
        parser.error("gate requires at least 50 samples and every startup/retrieval operation")
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="sqnic-auto-bench-") as temporary:
        root = Path(temporary).resolve()
        repo = root / "repo"
        repo.mkdir()
        db = root / "context.sqlite3"
        subprocess.run(["git", "init", "-q", str(repo)], check=True, timeout=30)
        if args.worktree_files:
            for index in range(args.worktree_files):
                path = repo / "files" / str(index // 100) / f"{index}.txt"
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("tracked benchmark fixture\n")
            subprocess.run(["git", "-C", str(repo), "add", "files"], check=True, timeout=30)
            subprocess.run(["git", "-C", str(repo), "-c", "user.name=Benchmark", "-c", "user.email=fixture@example.invalid", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "commit", "-qm", "fixture"], check=True, timeout=60)
        base = [str(binary), "--db", str(db)]

        def run(*command: str, payload: dict | None = None) -> tuple[dict, float, int]:
            if args.cold_cache:
                for path in (db, Path(str(db) + "-wal"), Path(str(db) + "-shm"), repo / "bench.jsonl"):
                    if path.is_file():
                        with path.open("rb") as stream:
                            os.fsync(stream.fileno())
                            os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
            start = time.perf_counter_ns()
            result = subprocess.run(
                [*base, *command],
                input=None if payload is None else json.dumps(payload).encode(),
                capture_output=True,
                check=True,
                timeout=120,
            )
            value = json.loads(result.stdout)
            if "systemMessage" in value:
                raise RuntimeError(value["systemMessage"])
            return value, (time.perf_counter_ns() - start) / 1e6, len(result.stdout)

        run("unpause", "--repo", str(repo))
        # Keep the workload deterministic: this fixture owns an artificial worker lease.
        # All ingestion is performed by measured foreground or record --once calls.
        with closing(sqlite3.connect(db)) as conn, conn:
            conn.execute(
                "INSERT INTO auto_leases VALUES(?, 'benchmark', unixepoch()+3600)",
                (str(repo),),
            )
        transcript = repo / "bench.jsonl"
        with transcript.open("w") as stream:
            for i in range(args.events):
                stream.write(
                    json.dumps(
                        {
                            "type": "user",
                            "sessionId": "bench",
                            "cwd": str(repo),
                            "uuid": f"event-{i}",
                            "message": {
                                "role": "user",
                                "content": f"record {i} retry-marker-{i % 100}: preserve this requirement",
                            },
                        }
                    )
                    + "\n"
                )
        payload = {
            "session_id": "bench",
            "cwd": str(repo),
            "transcript_path": str(transcript),
            "hook_event_name": "SessionStart",
        }
        hook = ("hook", "--repo", str(repo), "--harness", "claude")
        first, initial_ms, _ = run(*hook, payload=payload)
        if "hookSpecificOutput" not in first:
            raise RuntimeError("startup did not inject context")
        _, catchup_ms, _ = run("record", "--repo", str(repo), "--once")
        initial_peak_child_rss = None
        if platform.system() in {"Darwin", "Linux"}:
            import resource
            initial_peak_child_rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
            if platform.system() == "Linux":
                initial_peak_child_rss *= 1024
        restored = json.loads(first["hookSpecificOutput"]["additionalContext"].split("\n", 1)[1])
        task = restored["task"]
        with closing(sqlite3.connect(db)) as conn, conn:
            branch = conn.execute("SELECT branch FROM auto_sessions WHERE native_id='bench'").fetchone()[0]
            for index in range(1, args.tasks):
                name = f"extra-{index}"
                conn.execute("INSERT INTO tasks(id,repo) VALUES(?,?)", (name,str(repo)))
                conn.execute("INSERT INTO auto_sessions(repo,harness,native_id,task,branch,seen) VALUES(?,'claude',?,?,?,unixepoch()+3600)", (str(repo),name,name,branch))
        run("record", "--repo", str(repo), "--once")
        measurements = {}
        appended = 0

        def append_record() -> None:
            nonlocal appended
            with transcript.open("a") as stream:
                stream.write(
                    json.dumps(
                        {
                            "type": "user",
                            "sessionId": "bench",
                            "cwd": str(repo),
                            "uuid": f"append-{appended}",
                            "message": {
                                "role": "user",
                                "content": f"new requirement {appended}",
                            },
                        }
                    )
                    + "\n"
                )
            appended += 1

        commands = {
            "startup_hook": (hook, payload),
            "prompt_hook": (hook, {**payload, "hook_event_name": "UserPromptSubmit"}),
            "restore": (("restore", "--repo", str(repo), "--task", task), None),
            "restore_query": (
                ("restore", "--repo", str(repo), "--task", task, "--query", f"retry-marker-{min(42, args.events - 1)}"),
                None,
            ),
            "read_many": (("--read-only", "read-many", task, "1"), None),
            "search": (("--read-only", "search", task, f"retry-marker-{min(42, args.events - 1)}", "--requests-only"), None),
            "request_history": (("--read-only", "history", task, "--requests-only", "--scope", branch, "--after", "1", "--before", str(args.events), "--limit", "20"), None),
            "status": (("auto-status", "--repo", str(repo)), None),
            "unchanged_reconcile": (("record", "--repo", str(repo), "--once"), None),
            "append_and_reconcile": (("record", "--repo", str(repo), "--once"), None),
        }
        for name, (command, input_payload) in commands.items():
            if args.operations and name not in args.operations:
                continue
            values = []
            for iteration in range(args.samples + 3):
                if name == "append_and_reconcile":
                    append_record()
                value = run(*command, payload=input_payload)
                response = value[0]
                if name == "startup_hook":
                    response = json.loads(response["hookSpecificOutput"]["additionalContext"].split("\n", 1)[1])
                if name in {"startup_hook", "restore", "restore_query"}:
                    if response.get("status") != "restored" or response.get("task") != task:
                        raise RuntimeError(f"{name} returned the wrong task or restore status")
                    if response.get("git", {}).get("checkpoint_stale") is not False:
                        raise RuntimeError(f"{name} returned a stale Git checkpoint")
                if name == "read_many" and not all(item.get("status") == "ok" and item.get("data", {}).get("id") == 1 for item in response.get("items", [])):
                    raise RuntimeError("read-many did not return the expected original")
                if name == "read_many" and not response.get("items"):
                    raise RuntimeError("read-many returned no original")
                if name == "request_history" and [item.get("data", {}).get("id") for item in response.get("items", [])] != list(range(2, min(22, args.events))):
                    raise RuntimeError("request history lost or mixed original requests")
                if name == "search" and not response.get("matches"):
                    raise RuntimeError("search lost the expected matching requests")
                if iteration >= 3:
                    values.append(value)
            measurements[name] = {
                **summary([v[1] for v in values]),
                "max_output_bytes": max(v[2] for v in values),
            }
        with closing(sqlite3.connect(db)) as conn, conn:
            events = conn.execute(
                "SELECT count(*) FROM events WHERE kind NOT IN ('checkpoint','commit')"
            ).fetchone()[0]
            actual = conn.execute("SELECT raw FROM events WHERE source IS NOT NULL ORDER BY source,line")
            with transcript.open() as stream:
                originals_match = all(
                    row is not None and line is not None and json.loads(row[0]) == json.loads(line)
                    for row, line in itertools.zip_longest(actual, stream)
                )
            if not originals_match:
                raise RuntimeError("stored originals differ from the source transcript")
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        if events != args.events + appended:
            raise RuntimeError(
                f"expected {args.events + appended} originals; found {events}"
            )
        peak_rss = {}
        if platform.system() == "Darwin":
            for name in ("restore", "restore_query", "read_many", "search", "request_history"):
                command, _ = commands[name]
                measured = subprocess.run(["/usr/bin/time", "-l", *base, *command], capture_output=True, text=True, check=True, timeout=120)
                for line in measured.stderr.splitlines():
                    if line.strip().endswith("maximum resident set size"):
                        peak_rss[name] = int(line.split()[0])
        result = {
            "workload": "synthetic Claude-shaped JSONL; one worktree; extra active tasks held by fixture timestamps; growing append queries; see cache_mode; CLI startup included; worker disabled by fixture lease",
            "cache_mode": "per-sample fixture-file eviction advised with fsync + POSIX_FADV_DONTNEED; system caches retained" if args.cold_cache else "warm fixture caches; a fresh CLI and SQLite connection per sample",
            "query_peak_rss_bytes": peak_rss,
            "binary_bytes": binary.stat().st_size,
            "initial_peak_child_rss_bytes": initial_peak_child_rss,
            "initial_ingest_records_per_second": round(args.events / ((initial_ms + catchup_ms) / 1000), 1),
            "active_tasks": args.tasks,
            "tracked_worktree_files": args.worktree_files,
            "platform": platform.platform(),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "samples": args.samples,
            "events": args.events,
            "final_events": events,
            "original_records_exact_match": originals_match,
            "transcript_bytes": transcript.stat().st_size,
            "database_bytes": db.stat().st_size,
            "initial_startup_ms": round(initial_ms, 3),
            "initial_background_catchup_ms": round(catchup_ms, 3),
            "measurements": measurements,
        }
        result["latency_gate"] = {
            name: {"p95_ms": measurements[name]["p95_ms"], "limit_ms": 250 if name == "startup_hook" else 100, "passed": measurements[name]["p95_ms"] < (250 if name == "startup_hook" else 100)}
            for name in sorted(required) if name in measurements
        }
        result["gate_enforced"] = args.gate
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
        if args.gate and not all(check["passed"] for check in result["latency_gate"].values()):
            raise SystemExit("latency gate failed; inspect report")


if __name__ == "__main__":
    main()
