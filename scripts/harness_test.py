"""Run opt-in Claude Code and Codex MCP handoff checks using local synthetic data.

Uses existing harness authentication and may consume model usage.
Does not modify global harness configuration or use personal conversation history.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import secrets
import subprocess
import tempfile
import time
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/sqnic"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=["legacy", "bundle"], default="bundle")
    parser.add_argument("--reverse", action="store_true")
    parser.add_argument("--claude-model", default="claude-opus-4-5-20251101")
    parser.add_argument("--codex-model", default="gpt-5.6-luna")
    parser.add_argument("--profile", choices=["full", "handoff"], default="full")
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="sqnic-harness-") as temporary:
        root = Path(temporary)
        db = root / "context.sqlite3"
        base = [str(binary), "--db", str(db)]

        def run(command: list[str], **kwargs) -> subprocess.CompletedProcess:
            return subprocess.run(
                command,
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
                timeout=240,
                **kwargs,
            )

        def tool(*command: str) -> dict:
            return json.loads(run([*base, *command]).stdout)

        def git(*command: str) -> str:
            return run(["git", *command]).stdout.strip()

        git("init", "-q")
        git("config", "user.name", "Harness Fixture")
        git("config", "user.email", "fixture@example.invalid")
        (root / "retry.conf").write_text("retry_limit=3\n")
        git("add", "retry.conf")
        git(
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "set retry limit",
        )
        (root / "retry.conf").write_text("retry_limit=7\n")
        git("add", "retry.conf")
        git(
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "increase retry limit after controlled verification",
        )
        head = git("rev-parse", "HEAD")
        marker = "verified-" + secrets.token_hex(6)
        handoff = "handoff-" + secrets.token_hex(6)
        tool("create", "recovery-test", "--repo", str(root))
        history = root / "source.jsonl"
        with history.open("w") as stream:
            for i in range(120):
                if i == 15:
                    record = {
                        "type": "user",
                        "message": {
                            "content": "Rejected approach: unlimited retries. It can hide persistent failures."
                        },
                    }
                elif i == 31:
                    record = {
                        "type": "user",
                        "message": {
                            "content": [
                                {
                                    "type": "tool_result",
                                    "tool_use_id": "validation-1",
                                    "content": f"verification result: 23 tests passed; evidence marker {marker}",
                                }
                            ]
                        },
                    }
                else:
                    record = {
                        "type": "assistant",
                        "message": {"content": f"routine progress record {i}"},
                    }
                stream.write(json.dumps(record) + "\n")
        tool("import", "recovery-test", str(history), "--format", "claude")
        history.unlink()  # Recovery must use the imported copy, not the source file.
        tool(
            "update",
            "recovery-test",
            "--kind",
            "constraint",
            "--key",
            "retry",
            "--text",
            "retry limit is 3",
        )
        tool(
            "update",
            "recovery-test",
            "--kind",
            "constraint",
            "--key",
            "retry",
            "--text",
            "current retry limit is 7",
        )
        tool("git-sync", "recovery-test")
        server_args = ["--db", str(db), "serve"]
        if args.profile == "handoff":
            server_args.extend(["--profile", "handoff"])
        config = root / "mcp.json"
        config.write_text(
            json.dumps(
                {
                    "mcpServers": {
                        "sqnic": {
                            "command": str(binary),
                            "args": server_args,
                        }
                    }
                }
            )
        )
        prompt = (
            (
                "Use only sqnic MCP tools for task recovery-test. "
                + (
                    "Start with sqnic_evidence with query retry verification rejected. Use read_many to expand the needed original events together. "
                    if args.mode == "bundle"
                    else "Start with resume. "
                )
            )
            + "Recover the current retry limit (not a superseded value), rejected approach and reason, "
            "exact verification result and its evidence marker from older tool history, and the latest "
            "commit full hash, changed file, and changed value from its diff. Search and read original "
            "records for evidence. Return ONLY a JSON object, without prose or Markdown, with exactly these fields: "
            "retry_limit (integer), superseded_retry_limit (integer), rejected_approach (string), "
            "rejected_reason (string, quote the reason verbatim), verification_result (string), verification_marker (string), "
            "verification_event (integer), state_event (integer), commit_hash (string), changed_file (string), "
            "before (integer), after (integer), transfer_marker (string). "
        )
        report = {
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "versions": {
                name: run([name, "--version"]).stdout.strip()
                for name in ("claude", "codex")
            },
            "fixture": {
                "expected_head": head,
                "expected_verification_marker": marker,
                "expected_handoff_marker": handoff,
            },
            "runs": [],
        }

        def record(name: str, command: list[str]) -> str:
            start = time.perf_counter()
            result = subprocess.run(
                command,
                cwd=root,
                capture_output=True,
                text=True,
                timeout=240,
                stdin=subprocess.DEVNULL,
                check=False,
            )
            entry = {
                "harness": name,
                "exit_code": result.returncode,
                "elapsed_seconds": round(time.perf_counter() - start, 3),
                "stdout": result.stdout,
                "stderr": result.stderr,
            }
            report["runs"].append(entry)
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{name} failed; inspect {args.output}")
            return result.stdout

        claude_command = [
            "claude",
            "-p",
            "--model",
            args.claude_model,
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            str(config),
            "--permission-mode",
            "dontAsk",
            "--allowedTools",
            "mcp__sqnic",
            "--setting-sources",
            "",
            "--no-session-persistence",
        ]
        codex_command = [
            "codex",
            "exec",
            "--model",
            args.codex_model,
            "--ignore-user-config",
            "--json",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "-C",
            str(root),
            "-c",
            f"mcp_servers.sqnic.command={json.dumps(str(binary))}",
            "-c",
            f"mcp_servers.sqnic.args={json.dumps(server_args)}",
        ]
        outputs = {}
        commands = {"claude": claude_command, "codex": codex_command}
        order = ["codex", "claude"] if args.reverse else ["claude", "codex"]
        report["models"] = {"claude": args.claude_model, "codex": args.codex_model}
        report["mode"] = args.mode
        report["profile"] = args.profile
        report["order"] = order
        for index, name in enumerate(order):
            instruction = (
                f" Then save a progress note with key transfer and text {handoff}. Return it as transfer_marker."
                if index == 0
                else " Also recover the transfer progress marker left by the previous harness. Do not use shell commands or read files directly."
            )
            outputs[name] = record(name, [*commands[name], "--", prompt + instruction])
            log = root / f"{name}-output.jsonl"
            log.write_text(outputs[name])
            report[f"{name}_live_records_imported"] = tool(
                "import", "recovery-test", str(log), "--format", name
            )["added"]
        claude_output, codex_output = outputs["claude"], outputs["codex"]
        checks = {}
        for name, output in [("claude", claude_output), ("codex", codex_output)]:
            final = ""
            calls = []
            unexpected_shell = False
            usage = None
            for line in output.splitlines():
                event = json.loads(line)
                if event.get("type") == "result":
                    final = event.get("result", "")
                    usage = event.get("usage")
                if event.get("type") == "assistant":
                    calls.extend(
                        block["name"]
                        for block in event.get("message", {}).get("content", [])
                        if block.get("type") == "tool_use"
                    )
                if event.get("type") == "item.completed":
                    item = event.get("item", {})
                    if item.get("type") == "agent_message":
                        final = item.get("text", "")
                    if item.get("type") == "mcp_tool_call":
                        calls.append(item.get("tool"))
                    if item.get("type") == "command_execution":
                        unexpected_shell = True
                if event.get("type") == "turn.completed":
                    usage = event.get("usage")
            try:
                answer = json.loads(final)
                structured = isinstance(answer, dict)
            except json.JSONDecodeError:
                answer = {}
                structured = False
            if not isinstance(answer, dict):
                answer = {}
            checks[name] = {
                "verification_value": answer.get("verification_result")
                == "23 tests passed",
                "verification_marker_value": answer.get("verification_marker")
                == marker,
                "commit_value": answer.get("commit_hash") == head,
                "transfer_value": answer.get("transfer_marker") == handoff,
                "file_value": answer.get("changed_file") == "retry.conf",
                "rejected_value": answer.get("rejected_approach", "").lower()
                == "unlimited retries",
                "structured_answer": structured,
                "current_value": answer.get("retry_limit") == 7,
                "superseded_value": answer.get("superseded_retry_limit") == 3,
                "state_evidence": answer.get("state_event") == 122,
                "verification_evidence": answer.get("verification_event") == 32,
                "reason_supported": "hide persistent failures"
                in answer.get("rejected_reason", "").lower(),
                "diff_values": answer.get("before") == 3 and answer.get("after") == 7,
                "verification_marker_in_final": marker in final,
                "commit_hash_in_final": head in final,
                "handoff_marker_in_final": handoff in final,
                "verification_result_in_final": "23 tests passed" in final,
                "rejected_approach_in_final": "unlimited retries" in final.lower(),
                "changed_file_in_final": "retry.conf" in final,
                "mcp_calls_observed": bool(calls),
                "no_shell_calls": not unexpected_shell,
            }
            run_entry = next(
                entry for entry in report["runs"] if entry["harness"] == name
            )
            run_entry["final"] = final
            run_entry["tool_calls"] = calls
            run_entry["usage"] = usage
        report["checks"] = checks
        report["limits"] = (
            "Typed values, source IDs, reasons, diff values and observed tool calls are checked. Synthetic factual handoff only; no broad coding-continuation or automatic discovery claim. No personal history was used."
        )
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"checks": checks, "output": str(args.output)}, indent=2))
        if not all(all(values.values()) for values in checks.values()):
            raise RuntimeError("handoff checks failed; inspect the saved report")


if __name__ == "__main__":
    main()
