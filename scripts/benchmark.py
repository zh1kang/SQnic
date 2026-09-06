"""Measure the actual release CLI and persistent MCP on disposable local data."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def summary(samples: list[float]) -> dict[str, float]:
    ordered = sorted(samples)

    def percentile(p: float) -> float:
        return round(ordered[max(0, math.ceil(len(ordered) * p) - 1)], 3)

    return {
        "p50_ms": percentile(0.5),
        "p95_ms": percentile(0.95),
        "p99_ms": percentile(0.99),
        "mean_ms": round(statistics.mean(samples), 3),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/sqnic"))
    parser.add_argument("--events", type=int, default=10000)
    parser.add_argument("--samples", type=int, default=50)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--extended",
        action="store_true",
        help="measure schema-v3 evidence and capture tools",
    )
    args = parser.parse_args()
    if args.events < 20 or args.samples < 2:
        parser.error("use at least 20 events and 2 samples")
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="sqnic-bench-") as temporary:
        root = Path(temporary)
        db = root / "context.sqlite3"
        base = [str(binary), "--db", str(db)]

        def run(*command: str) -> tuple[object, float, int]:
            start = time.perf_counter_ns()
            result = subprocess.run(
                [*base, *command], check=True, capture_output=True, timeout=60
            )
            elapsed = (time.perf_counter_ns() - start) / 1e6
            return json.loads(result.stdout), elapsed, len(result.stdout)

        def git(*command: str) -> None:
            subprocess.run(
                ["git", "-C", str(root), *command],
                check=True,
                capture_output=True,
                timeout=30,
            )

        run("create", "bench", "--repo", str(root))
        git("init", "-q")
        git("config", "user.name", "Benchmark")
        git("config", "user.email", "benchmark@example.invalid")
        for i in range(20):
            (root / "sample.txt").write_text(f"revision {i}\n")
            git("add", "sample.txt")
            git(
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                f"sample change {i}",
            )
        history = root / "history.jsonl"
        with history.open("w") as stream:
            for i in range(args.events):
                record = {
                    "type": "message",
                    "id": f"e{i}",
                    "parentId": f"e{i - 1}",
                    "message": {
                        "role": "user" if i % 5 == 0 else "toolResult",
                        "content": [
                            {
                                "type": "text",
                                "text": f"shard{i % 100} event {i}: "
                                + "observed build result and exact task constraints. "
                                * 16,
                            }
                        ],
                    },
                }
                stream.write(json.dumps(record) + "\n")
        _, import_ms, _ = run("import", "bench", str(history))
        _, reimport_ms, _ = run("import", "bench", str(history))
        _, git_ms, _ = run("git-sync", "bench")
        run(
            "update",
            "bench",
            "--kind",
            "goal",
            "--text",
            "preserve task constraints and verify commit evidence",
        )
        commit_list, _, _ = run("commits", "bench")
        commit_hash = commit_list["commits"][0]["hash"]
        commands = {
            "tasks": ["tasks"],
            "resume": ["resume", "bench"],
            "search_selective": ["search", "bench", "shard42"],
            "search_common": ["search", "bench", "observed"],
            "search_missing": ["search", "bench", "absent-needle"],
            "history": ["history", "bench"],
            "read": ["read", "bench", "1"],
            "notes": ["notes", "bench"],
            "commits": ["commits", "bench"],
            "commit": ["commit", "bench", commit_hash],
            "commit_diff": ["commit", "bench", commit_hash, "--diff"],
            "stats": ["stats", "bench"],
        }
        if args.extended:
            commands.update(
                {
                    "evidence": ["evidence", "bench", "--query", "shard42"],
                    "read_many": ["read-many", "bench", "1,2,3"],
                    "checkpoints": ["checkpoints", "bench"],
                }
            )
        queries = {}
        for name, command in commands.items():
            _, first_ms, _ = run(*command)
            samples = [run(*command) for _ in range(args.samples)]
            queries[name] = {
                **summary([sample[1] for sample in samples]),
                "output_bytes": samples[-1][2],
                "first_process_ms": round(first_ms, 3),
            }
        writes = []
        for i in range(args.samples):
            _, ms, _ = run(
                "update", "bench", "--kind", "progress", "--text", f"iteration {i}"
            )
            writes.append(ms)
        with history.open("a") as stream:
            stream.write(json.dumps({"text": "incremental marker"}) + "\n")
        _, append_ms, _ = run("import", "bench", str(history))
        mcp = subprocess.Popen(
            [*base, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )

        def rpc(identifier: int, method: str, params: dict) -> tuple[dict, float]:
            start = time.perf_counter_ns()
            mcp.stdin.write(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "id": identifier,
                        "method": method,
                        "params": params,
                    }
                )
                + "\n"
            )
            mcp.stdin.flush()
            result = json.loads(mcp.stdout.readline())
            if "error" in result or result.get("result", {}).get("isError"):
                raise RuntimeError(result)
            return result, (time.perf_counter_ns() - start) / 1e6

        try:
            rpc(
                1,
                "initialize",
                {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "bench", "version": "1"},
                },
            )
            mcp.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
            mcp.stdin.flush()
            persistent = {}
            tool_cases = [
                ("tasks", {}),
                ("resume", {"task": "bench"}),
                ("search", {"task": "bench", "query": "shard42"}),
                ("history", {"task": "bench"}),
                ("read", {"task": "bench", "id": 1}),
                ("notes", {"task": "bench"}),
                ("commits", {"task": "bench"}),
                ("commit", {"task": "bench", "hash": commit_hash}),
                ("stats", {"task": "bench"}),
            ]
            if args.extended:
                tool_cases.extend(
                    [
                        ("evidence", {"task": "bench", "query": "shard42"}),
                        ("read_many", {"task": "bench", "refs": ["1", "2", "3"]}),
                        ("checkpoints", {"task": "bench"}),
                        (
                            "capture",
                            {
                                "task": "bench",
                                "key": "benchmark-retry",
                                "record": '{"text":"capture retry"}',
                            },
                        ),
                        (
                            "enrich",
                            {
                                "task": "bench",
                                "id": 1,
                                "text": "derived benchmark key",
                                "author": "benchmark",
                            },
                        ),
                        (
                            "link",
                            {
                                "task": "bench",
                                "id": 1,
                                "hash": commit_hash,
                                "relation": "supports",
                                "author": "benchmark",
                            },
                        ),
                    ]
                )
            for name, arguments in tool_cases:
                values = [
                    rpc(
                        i + 2,
                        "tools/call",
                        {"name": f"sqnic_{name}", "arguments": arguments},
                    )[1]
                    for i in range(args.samples)
                ]
                persistent[name] = summary(values)
            if args.extended:
                new_capture = []
                for i in range(args.samples):
                    response, elapsed = rpc(
                        args.samples + i + 2,
                        "tools/call",
                        {
                            "name": "sqnic_capture",
                            "arguments": {
                                "task": "bench",
                                "key": f"new-record-{i}",
                                "record": json.dumps({"text": f"new capture {i}"}),
                            },
                        },
                    )
                    payload = json.loads(response["result"]["content"][0]["text"])
                    if payload["added"] is not True:
                        raise RuntimeError(
                            "new capture measurement did not insert a record"
                        )
                    new_capture.append(elapsed)
                persistent["capture_new_record"] = summary(new_capture)
        finally:
            mcp.stdin.close()
            mcp.wait(timeout=10)
            mcp.stdout.close()
        peak_rss = {}
        if platform.system() == "Darwin":
            for name, command in [
                ("resume", ["resume", "bench"]),
                ("search_common", ["search", "bench", "observed"]),
            ]:
                measured = subprocess.run(
                    ["/usr/bin/time", "-l", *base, *command],
                    check=True,
                    capture_output=True,
                    text=True,
                    timeout=60,
                )
                for line in measured.stderr.splitlines():
                    if line.strip().endswith("maximum resident set size"):
                        peak_rss[name] = int(line.split()[0])
        stats, _, _ = run("stats", "bench")
        report = {
            "environment": {
                "platform": platform.platform(),
                "processor": platform.processor(),
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "binary_bytes": binary.stat().st_size,
            },
            "method": "release binary; synthetic text-heavy JSONL; warm filesystem caches; CLI includes process startup; MCP uses one process; no model calls",
            "events": args.events,
            "samples_per_query": args.samples,
            "history_bytes": history.stat().st_size,
            "initial_import_ms": round(import_ms, 3),
            "import_records_per_second": round(args.events / (import_ms / 1000)),
            "import_mib_per_second": round(
                history.stat().st_size / 1048576 / (import_ms / 1000), 3
            ),
            "unchanged_reimport_ms": round(reimport_ms, 3),
            "append_one_record_ms": round(append_ms, 3),
            "git_index_20_commits_ms": round(git_ms, 3),
            "queries": queries,
            "note_update": summary(writes),
            "persistent_mcp": persistent,
            "stats": stats,
            "query_peak_rss_bytes": peak_rss,
            "brief_vs_history_byte_reduction_percent": round(
                100 * (1 - queries["resume"]["output_bytes"] / history.stat().st_size),
                3,
            ),
            "limits": "byte reduction measures excerpts, not equivalent information or model quality; p99 uses limited samples; no claim about cold disk caches or other devices",
        }
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
