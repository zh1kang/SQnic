"""Opt-in, bounded coding acceptance for SQnic handoff history.

The live path invokes a real local harness only after ``--live`` is supplied.
All model output is kept in a private directory beside the requested report.
"""

from __future__ import annotations

import argparse
from contextlib import closing
import hashlib
import json
import math
import os
import platform
import queue
import re
import signal
import shlex
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any


MODEL = {"claude": "claude-opus-4-5-20251101", "codex": "gpt-5.6-luna"}
NEAR = (211, 29)
FAR = (347, 47)


def expected_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for zone in ("near", "far"):
        base, rate = NEAR if zone == "near" else FAR
        for weight in (0, 1, 99, 100, 101, 250, 501):
            cases.append({"fn": "quote_cents", "args": [weight, zone], "expected": base + rate * math.ceil(weight / 100)})
    for age, ttl in ((0, 0), (0, 1), (1, 1), (1, 2), (10, 9), (10, 10), (11, 10)):
        cases.append({"fn": "expired", "args": [age, ttl], "expected": age >= ttl})
    for values in ([], ["a"], ["a", "b", "a"], ["A", "a", "A"], ["x", "y", "x", "Y", "y"], ["", "", "a"]):
        cases.append({"fn": "stable_unique", "args": [values], "expected": list(dict.fromkeys(values))})
    return cases


def parse_json_events(text: str) -> dict[str, Any]:
    """Extract final text, usage and observed tool calls from harness JSONL."""
    final = ""
    usage: dict[str, int] = {}
    tools: list[str] = []
    failures: list[str] = []
    malformed_lines = 0
    usage_reports = 0
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            malformed_lines += 1
            continue
        if not isinstance(event, dict):
            continue
        if event.get("type") == "result":
            final = content_text(event.get("result", ""))
            usage_reports += 1
            usage.update(int_values(event.get("usage")))
            subtype = str(event.get("subtype", ""))
            if event.get("is_error") or subtype in {"error", "failure"} or subtype.startswith("error_"):
                failures.append(str(event.get("result", "harness result error")))
        if event.get("type") == "assistant":
            message = event.get("message", {})
            for block in message.get("content", []) if isinstance(message, dict) else []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    tools.append(str(block.get("name", "unknown")))
        if event.get("type") == "item.completed":
            item = event.get("item", {})
            if isinstance(item, dict):
                if item.get("type") == "agent_message":
                    final = content_text(item.get("text", ""))
                if item.get("type") == "mcp_tool_call":
                    tools.append(str(item.get("tool", item.get("name", "unknown"))))
                if item.get("type") == "command_execution":
                    tools.append("command_execution")
        if event.get("type") == "turn.completed":
            usage_reports += 1
            usage.update(int_values(event.get("usage")))
        if event.get("type") in {"error", "turn.failed"}:
            failures.append(content_text(event.get("message", event.get("error", "harness error"))))
    return {"final": final, "usage": usage, "tool_calls": tools, "failures": failures, "malformed_lines": malformed_lines, "usage_reports": usage_reports}


def content_text(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "".join(content_text(item.get("text", item.get("content", "")) if isinstance(item, dict) else item) for item in value)
    if value is None:
        return ""
    return str(value)


def int_values(value: Any) -> dict[str, int]:
    if not isinstance(value, dict):
        return {}
    return {key: int(raw) for key, raw in value.items() if isinstance(raw, (int, float)) and key.endswith("tokens")}


def parse_final_object(text: str) -> dict[str, Any] | None:
    candidate = text.strip()
    candidate = re.sub(r"^```(?:json)?\s*|\s*```$", "", candidate, flags=re.I | re.S).strip()
    try:
        value = json.loads(candidate)
    except json.JSONDecodeError:
        match = re.search(r"\{.*\}", candidate, re.S)
        if not match:
            return None
        try:
            value = json.loads(match.group(0))
        except json.JSONDecodeError:
            return None
    return value if isinstance(value, dict) else None


def oracle(answer: Path) -> dict[str, Any]:
    payload = json.dumps(expected_cases())
    code = """
import json, runpy, sys
mod = runpy.run_path(sys.argv[1])
cases = json.loads(sys.stdin.read())
results = []
for case in cases:
    try:
        value = mod[case['fn']](*case['args'])
        type_ok = type(value) is type(case['expected'])
        cents_ok = case['fn'] != 'quote_cents' or type(value) is int
        results.append({'ok': type_ok and cents_ok and value == case['expected'], 'value': value, 'expected': case['expected'], 'fn': case['fn'], 'type_ok': type_ok, 'cents_ok': cents_ok})
    except Exception as exc:
        results.append({'ok': False, 'error': type(exc).__name__ + ': ' + str(exc), 'expected': case['expected'], 'fn': case['fn']})
print(json.dumps(results))
"""
    completed = subprocess.run([sys.executable, "-S", "-c", code, str(answer)], input=payload, text=True, capture_output=True, timeout=20, check=False)
    if completed.returncode:
        return {"passed": False, "error": completed.stderr.strip() or f"oracle exit {completed.returncode}", "cases": []}
    try:
        cases = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        return {"passed": False, "error": f"oracle output: {exc}", "cases": []}
    return {"passed": bool(cases) and all(case.get("ok") for case in cases), "cases": cases, "case_count": len(cases)}


def validate_live(live: bool, harness: str) -> None:
    if harness != "none" and not live:
        raise ValueError("paid model harnesses require --live")


def run_sqnic(binary: Path, db: Path, args: list[str], *, stdin: bytes | None = None) -> tuple[dict[str, Any], bytes, float]:
    started = time.monotonic()
    result = subprocess.run([str(binary), "--db", str(db), *args], input=stdin, capture_output=True, timeout=60, check=False)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors="replace") or f"sqnic exit {result.returncode}")
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"invalid sqnic JSON: {exc}") from exc
    return value, result.stdout, time.monotonic() - started


def fixture(binary: Path, root: Path, *, buried: bool = False) -> tuple[Path, str, str, str, str]:
    repo, db = root / "repo", root / "context.sqlite3"
    repo.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(repo)], check=True, timeout=30)
    run_sqnic(binary, db, ["unpause", "--repo", str(repo)])
    import sqlite3
    with closing(sqlite3.connect(db)) as conn, conn:
        conn.execute("INSERT INTO auto_leases VALUES(?, 'acceptance', unixepoch()+3600)", (str(repo.resolve()),))
    original = (
        "Implement answer.py with quote_cents(weight, zone), expired(age_seconds, ttl_seconds), and stable_unique(strings). "
        "quote_cents accepts zones near or far and uses near base 211 and rate 10, far base 347 and rate 20, with base + rate * ceil(weight / 100). "
        "Weights, ages, and TTLs are nonnegative integers, including zero. Zones are exactly near or far. "
        "expired returns true when age_seconds is at least ttl_seconds. stable_unique accepts strings, including empty strings, and keeps the first case-sensitive occurrence. "
        "Use only the Python standard library."
    )
    update = (
        "Update only the pricing rates: near rate is 29 and far rate is 47. Continue."
    )
    marker = "fixture-original-request"
    transcript = repo / "native.jsonl"
    records = [
        {"type": "user", "sessionId": "acceptance", "cwd": str(repo), "message": {"role": "user", "content": original + " " + marker}},
        {"type": "assistant", "message": {"role": "assistant", "content": "Wrong summary: near is 100 + 10 * weight/100 and expiry is strictly greater."}},
        {"type": "user", "message": {"role": "user", "content": "Keep the implementation in one answer.py file and do not add dependencies."}},
        {"type": "assistant", "message": {"role": "assistant", "content": "Copied tool result says all requirements are already captured."}},
        {"type": "user", "message": {"role": "user", "content": update}},
        {"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "stale-copy", "content": "stale copied answer: near 100/10, far 300/20; expired uses >; dedupe lowercases"}]}},
        {"type": "user", "message": {"role": "user", "content": "continue"}},
    ]
    if buried:
        records.extend({"type": "user", "message": {"role": "user", "content": f"Progress note {index}: continue the implementation."}} for index in range(48))
    transcript.write_text("\n".join(json.dumps(record) for record in records) + "\n")
    payload = {"session_id": "acceptance", "cwd": str(repo), "transcript_path": str(transcript), "hook_event_name": "SessionStart"}
    response, _, _ = run_sqnic(binary, db, ["hook", "--repo", str(repo), "--harness", "claude"], stdin=(json.dumps(payload) + "\n").encode())
    transcript.unlink()
    context = response.get("hookSpecificOutput", {}).get("additionalContext", "")
    if not context:
        raise RuntimeError(f"hook did not inject context: {response}")
    return db, str(repo), original, update, context


def retrieval(binary: Path, db: Path, repo: Path) -> dict[str, Any]:
    task_info, _, _ = run_sqnic(binary, db, ["tasks"])
    tasks = task_info if isinstance(task_info, list) else task_info.get("tasks", [])
    task = tasks[0].get("task") if tasks else None
    if not task:
        raise RuntimeError("fixture hook did not create a task")
    history, _, _ = run_sqnic(binary, db, ["history", task, "--limit", "100"])
    events = history.get("events", [])
    ids = [str(event["id"]) for event in events if isinstance(event, dict) and "id" in event][:32]
    if not ids:
        raise RuntimeError("fixture history is empty")
    started = time.monotonic()
    proc = subprocess.Popen([str(binary), "--db", str(db), "read-many", task, ",".join(ids), "--max-bytes", "20000"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    output = bytearray()
    first_success: float | None = None
    assert proc.stdout is not None
    for line in proc.stdout:
        output.extend(line)
        if first_success is None:
            try:
                json.loads(line)
            except json.JSONDecodeError:
                continue
            first_success = time.monotonic() - started
    stderr = proc.stderr.read().decode(errors="replace") if proc.stderr else ""
    code = proc.wait(timeout=60)
    if proc.stdout:
        proc.stdout.close()
    if proc.stderr:
        proc.stderr.close()
    if code:
        raise RuntimeError(stderr or f"read-many exit {code}")
    return {"task": task, "output_bytes": len(output), "first_success_seconds": first_success, "output": output.decode(errors="replace")}


def prompt(context: str, original: str, update: str, direct: bool) -> str:
    source = f"Original specification:\n{original}\n\nLatest update:\n{update}" if direct else f"SQnic context replay (provided explicitly; native automatic discovery is not claimed):\n{context}"
    return source + "\n\nImplement only answer.py in the current repository. Do not commit. Use the exact requirements and write the file. Return a short JSON object with exact SQnic source event IDs as evidence, or an empty evidence list. Do not invent tests or change other files."


def harness_command(name: str, repo: Path, model_prompt: str) -> list[str]:
    if name == "claude":
        return ["claude", "-p", "--output-format", "stream-json", "--verbose", "--model", MODEL[name], "--tools", "Read,Write,Edit,Bash", "--allowedTools", "Read,Write,Edit,Bash", "--permission-mode", "acceptEdits", "--max-budget-usd", "1", "--max-turns", "12", "--setting-sources", "", "--no-session-persistence", "--", model_prompt]
    return ["codex", "exec", "--ignore-user-config", "--json", "--ephemeral", "--sandbox", "workspace-write", "-C", str(repo), "--model", MODEL[name], model_prompt]


def invokes_sqnic(command: str, executable: Path) -> bool:
    try:
        tokens = list(shlex.shlex(command, posix=True, punctuation_chars=True))
    except ValueError:
        return False
    # Shell wrappers carry the command as one quoted argument.
    if tokens and Path(tokens[0]).name in {"sh", "bash", "zsh"}:
        for index, token in enumerate(tokens[:-1]):
            if token in {"-c", "-lc"}:
                return invokes_sqnic(tokens[index + 1], executable)
    for index, token in enumerate(tokens):
        if token != str(executable) or (index and tokens[index - 1] not in {"&&", "||", ";", "|"}):
            continue
        end = next((i for i in range(index + 1, len(tokens)) if tokens[i] in {"&&", "||", ";", "|"}), len(tokens))
        if any(arg in {"read-many", "read", "search", "history", "evidence", "restore"} for arg in tokens[index + 1:end]):
            return True
    return False


def retrieval_payload(raw: str) -> str:
    """Extract actual records from SQnic JSON lines in a compound command output."""
    payloads = []
    for line in raw.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(value, dict):
            continue
        if isinstance(value.get("matches"), list) and "query" in value and "requests_only" in value:
            if any(isinstance(match, dict) and "id" in match for match in value["matches"]):
                payloads.append(line)
            continue
        if value.get("historical_data") is not True:
            continue
        items = value.get("items", [])
        events = value.get("events", [])
        has_items = isinstance(items, list) and any(
            isinstance(item, dict) and item.get("status") == "ok"
            and isinstance(item.get("data"), dict) and "id" in item["data"]
            for item in items
        )
        has_events = isinstance(events, list) and any(isinstance(event, dict) and "id" in event for event in events)
        if has_items or has_events:
            payloads.append(line)
    return "\n".join(payloads)


def retrieval_stats(name: str, text: str, executable: Path) -> dict[str, Any]:
    commands: list[str] = []
    failed: list[dict[str, Any]] = []
    successful: list[dict[str, Any]] = []
    pending: dict[str, str] = {}
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        if name == "claude" and event.get("type") == "assistant":
            for block in event.get("message", {}).get("content", []):
                if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == "Bash":
                    command = str(block.get("input", {}).get("command", ""))
                    if invokes_sqnic(command, executable):
                        pending[str(block.get("id", ""))] = command
                        commands.append(command)
        if name == "claude" and event.get("type") == "user":
            for block in event.get("message", {}).get("content", []):
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    continue
                command = pending.pop(str(block.get("tool_use_id", "")), None)
                if not command:
                    continue
                raw = content_text(block.get("content", ""))
                payload = retrieval_payload(raw)
                entry = {"command": command, "output_bytes": len(raw.encode()), "output": raw}
                if block.get("is_error"):
                    failed.append(entry)
                if payload:
                    successful.append({"command": command, "output_bytes": len(payload.encode()), "output": payload})
        if name == "codex" and event.get("type") == "item.completed":
            item = event.get("item", {})
            if not isinstance(item, dict) or item.get("type") != "command_execution":
                continue
            command = str(item.get("command", ""))
            if not invokes_sqnic(command, executable):
                continue
            commands.append(command)
            raw = str(item.get("aggregated_output", ""))
            payload = retrieval_payload(raw)
            entry = {"command": command, "output_bytes": len(raw.encode()), "output": raw}
            if item.get("exit_code", 0) not in (0, None):
                failed.append(entry)
            if payload:
                successful.append({"command": command, "output_bytes": len(payload.encode()), "output": payload})
    return {"commands": commands, "command_count": len(commands), "failed_commands": failed, "failed_command_count": len(failed), "retrieval_error_count": sum(not retrieval_payload(item["output"]) for item in failed), "successful_retrievals": successful, "successful_retrieval_count": len(successful), "retrieval_output_bytes": sum(item["output_bytes"] for item in successful), "first_successful_retrieval_seconds": None}


def run_harness(name: str, repo: Path, model_prompt: str, raw_dir: Path, executable: Path, direct: bool, parent_preflight: dict[str, Any]) -> dict[str, Any]:
    raw_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    started = time.monotonic()
    command = harness_command(name, repo, model_prompt)
    stderr_path = raw_dir / f"{name}.stderr"
    lines: queue.Queue[tuple[float, bytes | None]] = queue.Queue()

    def read_stdout(stream: Any) -> None:
        for line in iter(stream.readline, b""):
            lines.put((time.monotonic(), line))
        lines.put((time.monotonic(), None))

    try:
        stderr_handle = stderr_path.open("wb")
        try:
            process = subprocess.Popen(command, cwd=repo, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=stderr_handle, start_new_session=(os.name == "posix"))
        except OSError:
            stderr_handle.close()
            raise
        stderr_handle.close()
        assert process.stdout is not None
        reader = threading.Thread(target=read_stdout, args=(process.stdout,), daemon=True)
        reader.start()
        try:
            process.wait(timeout=150)
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait(timeout=10)
            reader.join(timeout=2)
            observed = iter_queue(lines)
            stdout = b"".join(item for _, item in observed if item is not None).decode(errors="replace")
            raw = raw_dir / f"{name}.jsonl"
            raw.write_text(stdout + "\n[TIMEOUT]\n")
            process.stdout.close()
            return {"harness": name, "model": MODEL[name], "command": command, "prompt_bytes": len(model_prompt.encode()), "exit_code": None, "timed_out": True, "elapsed_seconds": round(time.monotonic() - started, 3), "stderr": stderr_path.read_text(errors="replace"), "failures": ["timeout after 150 seconds"], "raw_log": str(raw), "malformed_lines": parse_json_events(stdout)["malformed_lines"], "retrieval": retrieval_stats(name, stdout, executable), "usage": {}, "first_pass_correct": False}
        reader.join(timeout=5)
        stream_complete = not reader.is_alive()
        if not stream_complete and os.name == "posix":
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            reader.join(timeout=2)
        observed = iter_queue(lines)
        stdout = b"".join(item for _, item in observed if item is not None).decode(errors="replace")
        (raw_dir / f"{name}.arrival-seconds.json").write_text(json.dumps([timestamp - started for timestamp, line in observed if line is not None]) + "\n")
        if not reader.is_alive():
            process.stdout.close()
        stderr = stderr_path.read_text(errors="replace")
        raw = raw_dir / f"{name}.jsonl"
        raw.write_text(stdout + ("\n[stderr]\n" + stderr if stderr else ""))
        parsed = parse_json_events(stdout)
        if not stream_complete:
            parsed["failures"].append("harness output stream did not complete")
        usage = parsed["usage"]
        cached = usage.get("cache_read_input_tokens", 0) + usage.get("cached_input_tokens", 0)
        created = usage.get("cache_creation_input_tokens", 0)
        input_tokens = usage.get("input_tokens", 0)
        total = input_tokens + cached + created if name == "claude" else input_tokens
        uncached = input_tokens + created if name == "claude" else max(0, total - cached)
        retrieval = retrieval_stats(name, stdout, executable)
        retrieval["first_successful_retrieval_seconds"] = first_retrieval_time_observed(name, observed, executable, started)
        return {"harness": name, "model": MODEL[name], "command": command, "prompt_bytes": len(model_prompt.encode()), "exit_code": process.returncode, "timed_out": False, "elapsed_seconds": round(time.monotonic() - started, 3), "stderr": stderr, "final": parsed["final"], "final_object": parse_final_object(parsed["final"]), "tool_calls": parsed["tool_calls"], "tool_count": len(parsed["tool_calls"]), "command_count": retrieval["command_count"], "failures": parsed["failures"], "malformed_lines": parsed["malformed_lines"], "usage": {"available": bool(usage) and parsed["usage_reports"] == 1, "report_count": parsed["usage_reports"], "aggregate_input_tokens_reported": total if usage else None, "cached_input_tokens_reported": cached if usage else None, "cache_creation_input_tokens_reported": created if usage else None, "uncached_input_tokens_reported": uncached if usage else None}, "retrieval": retrieval, "parent_preflight": parent_preflight, "raw_log": str(raw)}
    except OSError as exc:
        return {"harness": name, "model": MODEL[name], "command": command, "prompt_bytes": len(model_prompt.encode()), "exit_code": None, "timed_out": False, "elapsed_seconds": round(time.monotonic() - started, 3), "failures": [f"harness launch failed: {exc}"], "malformed_lines": 0, "retrieval": retrieval_stats(name, "", executable), "usage": {}, "parent_preflight": parent_preflight, "first_pass_correct": False}


def iter_queue(lines: queue.Queue[tuple[float, bytes | None]]) -> list[tuple[float, bytes | None]]:
    values: list[tuple[float, bytes | None]] = []
    while True:
        try:
            item = lines.get_nowait()
        except queue.Empty:
            return values
        values.append(item)
        if item[1] is None:
            return values


def first_retrieval_time_observed(name: str, observed: list[tuple[float, bytes | None]], executable: Path, started: float) -> float | None:
    pending: set[str] = set()
    for timestamp, raw_line in observed:
        if raw_line is None:
            continue
        line = raw_line.decode(errors="replace")
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if name == "claude" and event.get("type") == "assistant":
            for block in event.get("message", {}).get("content", []):
                if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == "Bash":
                    command = str(block.get("input", {}).get("command", ""))
                    if invokes_sqnic(command, executable):
                        pending.add(str(block.get("id", "")))
        if name == "claude" and event.get("type") == "user":
            for block in event.get("message", {}).get("content", []):
                if isinstance(block, dict) and block.get("type") == "tool_result" and str(block.get("tool_use_id", "")) in pending:
                    pending.discard(str(block.get("tool_use_id", "")))
                    if retrieval_payload(content_text(block.get("content", ""))):
                        return timestamp - started
        if name == "codex" and event.get("type") == "item.completed" and event.get("item", {}).get("type") == "command_execution":
            item = event["item"]
            if invokes_sqnic(str(item.get("command", "")), executable) and retrieval_payload(str(item.get("aggregated_output", ""))):
                return timestamp - started
    return None


def source_digest() -> str:
    import ast

    root = Path(__file__).resolve().parents[1]
    paths = [root / "Cargo.toml", root / "Cargo.lock", Path(__file__).resolve()]
    paths.extend(path for directory in ("src", "adapters") for path in (root / directory).rglob("*") if path.is_file())
    digest = hashlib.sha256()
    for path in sorted(paths):
        content = path.read_bytes()
        if path == Path(__file__).resolve():
            # Verifier-only fixes do not change the executed fixture or measurements.
            source = path.read_text()
            lines = source.splitlines()
            for node in reversed(ast.parse(source).body):
                if isinstance(node, ast.FunctionDef) and node.name in {"source_digest", "validate_report"}:
                    lines[node.lineno - 1:node.end_lineno] = [f"# report-only function: {node.name}"]
            content = "\n".join(lines).encode()
        digest.update(path.relative_to(root).as_posix().encode() + b"\0" + content + b"\0")
    return digest.hexdigest()


def validate_report(report: dict[str, Any]) -> list[str]:
    errors = []
    if report.get("source_sha256") != source_digest():
        errors.append("live evidence does not match the current source and runner")
    if report.get("live") is not True or report.get("buried_update") is not True or report.get("direct_spec_control") is not False:
        errors.append("release evidence must use live buried-change handoffs")
    repeats = report.get("repeats")
    if type(repeats) is not int or not 3 <= repeats <= 10:
        errors.append("release evidence requires 3..10 complete repeats")
    elif set(report.get("binaries", {})) != {f"candidate-run-{index + 1}" for index in range(repeats)}:
        errors.append("candidate repeat labels are incomplete or unexpected")
    counts = {name: 0 for name in MODEL}
    fixtures: set[str] = set()
    for label, binary in report.get("binaries", {}).items():
        if not re.fullmatch(r"candidate-run-[1-9][0-9]*", label):
            errors.append(f"{label}: only candidate repeats can satisfy the release gate")
            continue
        seen = set()
        for run in binary.get("runs", []):
            name = run.get("harness")
            if name not in MODEL or run.get("model") != MODEL.get(name) or name in seen:
                errors.append(f"{label}: unexpected or duplicate model run")
                continue
            seen.add(name)
            fixture_id = run.get("parent_preflight", {}).get("task")
            if not isinstance(fixture_id, str) or not fixture_id or fixture_id in fixtures:
                errors.append(f"{label}/{name}: missing or reused fixture identity")
            else:
                fixtures.add(fixture_id)
                counts[name] += 1
            oracle_result = run.get("oracle", {})
            cases = oracle_result.get("cases", [])
            if (run_failed(run, False) or run.get("failed") is not False or run.get("first_pass_correct") is not True
                    or run.get("timed_out") is not False or type(run.get("exit_code")) is not int
                    or oracle_result.get("passed") is not True
                    or run.get("required_evidence_complete") is not True
                    or run.get("measurement_complete") is not True or len(cases) != len(expected_cases())
                    or not all(case.get("ok") is True for case in cases)):
                errors.append(f"{label}/{name}: accuracy or measurement gate failed")
        if seen != set(MODEL):
            errors.append(f"{label}: a configured harness is missing")
    if any(count < 3 for count in counts.values()):
        errors.append("at least three independent runs of each configured harness are required")
    return errors


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/sqnic"))
    parser.add_argument("--baseline-binary", type=Path)
    parser.add_argument("--harness", choices=["claude", "codex", "both"], default="both")
    parser.add_argument("--direct-spec-control", action="store_true")
    parser.add_argument("--repeats", type=int, default=1, choices=range(1, 11))
    parser.add_argument("--buried-update", action="store_true", help="hide the rate change outside the startup batch behind 48 continuation requests")
    parser.add_argument("--live", action="store_true", help="allow paid model harness runs")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--check-report", type=Path, help="validate saved release evidence without calling models")
    return parser


def run_failed(result: dict[str, Any], direct: bool) -> bool:
    retrieval = result.get("retrieval", {})
    return bool(
        result.get("exit_code") != 0
        or result.get("timed_out")
        or bool(result.get("failures"))
        or result.get("required_evidence_complete") is False
        or result.get("measurement_complete") is False
        or retrieval.get("retrieval_error_count", 0) > 0
        or not result.get("oracle", {}).get("passed")
        or (not direct and retrieval.get("successful_retrieval_count", 0) == 0)
    )


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.check_report:
        errors = validate_report(json.loads(args.check_report.read_text()))
        print(json.dumps({"passed": not errors, "errors": errors}))
        return int(bool(errors))
    if not args.output:
        build_parser().error("--output is required for a live run")
    try:
        validate_live(args.live, args.harness)
    except ValueError as exc:
        build_parser().error(str(exc))
    binary = args.binary.resolve()
    if not binary.is_file():
        build_parser().error(f"binary does not exist: {binary}")
    args.output.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    raw_dir = args.output.parent / f".{args.output.stem}.raw"
    raw_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    report: dict[str, Any] = {"source_sha256": source_digest(), "platform": platform.platform(), "live": args.live, "direct_spec_control": args.direct_spec_control, "buried_update": args.buried_update, "repeats": args.repeats, "binaries": {}}
    overall_failed = False
    with tempfile.TemporaryDirectory(prefix="sqnic-handoff-acceptance-") as temporary:
        binaries = [("candidate", binary)]
        if args.baseline_binary:
            binaries.append(("baseline", args.baseline_binary.resolve()))
        binaries = [(f"{label}-run-{trial + 1}" if args.repeats > 1 else label, selected) for label, selected in binaries for trial in range(args.repeats)]
        for label, selected_binary in binaries:
            report["binaries"][label] = {"binary": str(selected_binary), "binary_sha256": hashlib.sha256(selected_binary.read_bytes()).hexdigest(), "runs": []}
            for name in (["claude", "codex"] if args.harness == "both" else [args.harness]):
                root = Path(temporary) / label / name
                try:
                    db, repo_name, original, update, context = fixture(selected_binary, root, buried=args.buried_update)
                    retrieved = retrieval(selected_binary, db, Path(repo_name))
                    preflight = {key: value for key, value in retrieved.items() if key != "output"}
                    preflight["startup_bytes"] = 0 if args.direct_spec_control else len(context.encode())
                    preflight["startup_estimated_tokens_bytes_div_4"] = preflight["startup_bytes"] / 4
                    preflight["direct_control"] = args.direct_spec_control
                    model_context = "" if args.direct_spec_control else context
                    result = run_harness(name, Path(repo_name), prompt(model_context, original, update, args.direct_spec_control), raw_dir / label / name, selected_binary, args.direct_spec_control, preflight)
                    answer = Path(repo_name) / "answer.py"
                    result["oracle"] = oracle(answer) if answer.is_file() else {"passed": False, "error": "answer.py missing"}
                    payloads = "\n".join(item["output"] for item in result.get("retrieval", {}).get("successful_retrievals", []))
                    final = result.get("final_object") or {}
                    evidence = {str(item) for item in final.get("evidence", [])} if isinstance(final.get("evidence"), list) else set()
                    result["required_evidence_complete"] = args.direct_spec_control or (
                        "fixture-original-request" in payloads and update in payloads and {"1", "5"}.issubset(evidence)
                    )
                    result["measurement_complete"] = result.get("usage", {}).get("available", False) and result.get("malformed_lines", 0) == 0
                    result["failed"] = run_failed(result, args.direct_spec_control)
                    result["first_pass_correct"] = not result["failed"]
                    overall_failed |= result["failed"]
                    report["binaries"][label]["runs"].append(result)
                    report["binaries"][label].setdefault("preflight", preflight)
                    args.output.write_text(json.dumps(report, indent=2) + "\n")
                except Exception as exc:
                    report["binaries"][label]["runs"].append({"harness": name, "failed": True, "fixture_error": f"{type(exc).__name__}: {exc}"})
                    overall_failed = True
                    args.output.write_text(json.dumps(report, indent=2) + "\n")
    report["fixture_errors"] = sum(
        "fixture_error" in run
        for value in report["binaries"].values()
        for run in value["runs"]
    )
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    run_count = sum(len(value["runs"]) for value in report["binaries"].values())
    print(json.dumps({"output": str(args.output), "runs": run_count, "fixture_errors": report["fixture_errors"]}))
    return 0 if report["fixture_errors"] == 0 and not overall_failed else 1


if __name__ == "__main__":
    raise SystemExit(main())
