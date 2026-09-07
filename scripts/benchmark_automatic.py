"""Measure automatic handoff on disposable native transcript fixtures, without a model."""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path

from benchmark import summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/sqnic"))
    parser.add_argument("--events", type=int, default=10000)
    parser.add_argument("--samples", type=int, default=50)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.events < 20 or args.samples < 2:
        parser.error("use at least 20 events and 2 samples")
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="sqnic-auto-bench-") as temporary:
        root = Path(temporary).resolve()
        repo = root / "repo"
        repo.mkdir()
        db = root / "context.sqlite3"
        subprocess.run(["git", "init", "-q", str(repo)], check=True, timeout=30)
        base = [str(binary), "--db", str(db)]

        def run(*command: str, payload: dict | None = None) -> tuple[dict, float, int]:
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
        with sqlite3.connect(db) as conn:
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
            "restore": (("restore", "--repo", str(repo)), None),
            "restore_query": (
                ("restore", "--repo", str(repo), "--query", "retry-marker-42"),
                None,
            ),
            "status": (("auto-status", "--repo", str(repo)), None),
            "unchanged_reconcile": (("record", "--repo", str(repo), "--once"), None),
            "append_and_reconcile": (("record", "--repo", str(repo), "--once"), None),
        }
        for name, (command, input_payload) in commands.items():
            values = []
            for iteration in range(args.samples + 3):
                if name == "append_and_reconcile":
                    append_record()
                value = run(*command, payload=input_payload)
                if iteration >= 3:
                    values.append(value)
            measurements[name] = {
                **summary([v[1] for v in values]),
                "max_output_bytes": max(v[2] for v in values),
            }
        with sqlite3.connect(db) as conn:
            events = conn.execute(
                "SELECT count(*) FROM events WHERE kind != 'checkpoint'"
            ).fetchone()[0]
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        if events != args.events + appended:
            raise RuntimeError(
                f"expected {args.events + appended} originals; found {events}"
            )
        result = {
            "workload": "synthetic Claude-shaped JSONL; one worktree/session, unchanged warm queries; CLI process startup included; worker disabled by fixture lease",
            "platform": platform.platform(),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "samples": args.samples,
            "events": args.events,
            "final_events": events,
            "transcript_bytes": transcript.stat().st_size,
            "database_bytes": db.stat().st_size,
            "initial_startup_ms": round(initial_ms, 3),
            "initial_background_catchup_ms": round(catchup_ms, 3),
            "measurements": measurements,
        }
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
