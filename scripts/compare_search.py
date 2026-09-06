"""Paired warm-process FTS measurements across task counts on equivalent disposable data."""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import tempfile
import time
from pathlib import Path

from benchmark import summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, default=Path("target/release/sqnic"))
    parser.add_argument("--events", type=int, default=100000)
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.events < 100 or args.samples < 10:
        parser.error("use at least 100 events and 10 samples")
    results = []
    with tempfile.TemporaryDirectory(prefix="sqnic-compare-") as temp:
        root = Path(temp)
        for count in (1, 10, 100):
            for label, binary in (
                ("before", args.before.resolve()),
                ("after", args.after.resolve()),
            ):
                db = root / f"{count}-{label}.sqlite"
                subprocess.run(
                    [
                        str(binary),
                        "--db",
                        str(db),
                        "create",
                        "task0",
                        "--repo",
                        str(root),
                    ],
                    check=True,
                    capture_output=True,
                )
                # Same FTS corpus without importer cost. Timings below exclude fixture construction.
                with sqlite3.connect(db) as conn:
                    conn.executemany(
                        "INSERT INTO tasks(id,repo) VALUES(?,?)",
                        [(f"task{i}", str(root)) for i in range(1, count)],
                    )
                    conn.executemany(
                        "INSERT INTO events(task,kind,body,raw) VALUES(?,?,?,?)",
                        (
                            (
                                f"task{i % count}",
                                "text",
                                f"common progress shard{i % 97} event{i}",
                                "{}",
                            )
                            for i in range(args.events)
                        ),
                    )
                    conn.execute("INSERT INTO event_fts(event_fts) VALUES('optimize')")
                with subprocess.Popen(
                    [str(binary), "--db", str(db), "serve"],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    text=True,
                ) as proc:

                    def rpc(method: str, params: dict) -> dict:
                        proc.stdin.write(
                            json.dumps(
                                {
                                    "jsonrpc": "2.0",
                                    "id": 1,
                                    "method": method,
                                    "params": params,
                                }
                            )
                            + "\n"
                        )
                        proc.stdin.flush()
                        reply = json.loads(proc.stdout.readline())
                        if "error" in reply or reply.get("result", {}).get("isError"):
                            raise RuntimeError(reply)
                        return reply

                    rpc("initialize", {})
                    for query in ("common", "shard42", "missing"):
                        payload = {
                            "name": "sqnic_search",
                            "arguments": {"task": "task0", "query": query},
                        }
                        for _ in range(5):
                            rpc("tools/call", payload)
                        times = []
                        for _ in range(args.samples):
                            start = time.perf_counter_ns()
                            rpc("tools/call", payload)
                            times.append((time.perf_counter_ns() - start) / 1e6)
                        results.append(
                            {
                                "version": label,
                                "tasks": count,
                                "query": query,
                                **summary(times),
                            }
                        )
                    proc.stdin.close()
                    proc.wait(timeout=10)
    report = {
        "method": "sequential before/after binaries; same 100%-controlled synthetic FTS bodies; persistent MCP; warm caches; five warmups; FTS optimized after load; no model calls",
        "events": args.events,
        "samples": args.samples,
        "results": results,
        "limits": "Synthetic SQL fixture isolates search, not import or real retrieval quality. Sequential workstation measurements are not a controlled hardware lab.",
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
