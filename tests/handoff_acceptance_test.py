import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from scripts import handoff_acceptance as acceptance

ROOT = Path(__file__).parents[1]


class HandoffAcceptanceTest(unittest.TestCase):
    def test_oracle_catches_boundary_and_case_errors(self):
        with tempfile.TemporaryDirectory() as directory:
            answer = Path(directory) / "answer.py"
            answer.write_text(
                "def quote_cents(weight, zone):\n"
                "    return (211 if zone == 'near' else 347) + (29 if zone == 'near' else 47) * (weight // 100)\n"
                "def expired(age_seconds, ttl_seconds):\n"
                "    return age_seconds > ttl_seconds\n"
                "def stable_unique(strings):\n"
                "    return list(dict.fromkeys(value.lower() for value in strings))\n"
            )
            result = acceptance.oracle(answer)
            self.assertFalse(result["passed"])
            self.assertGreaterEqual(result["case_count"], 20)

    def test_oracle_accepts_exact_spec(self):
        with tempfile.TemporaryDirectory() as directory:
            answer = Path(directory) / "answer.py"
            answer.write_text(
                "import math\n"
                "def quote_cents(weight, zone):\n"
                "    base, rate = ((211, 29) if zone == 'near' else (347, 47))\n"
                "    return base + rate * math.ceil(weight / 100)\n"
                "def expired(age_seconds, ttl_seconds):\n"
                "    return age_seconds >= ttl_seconds\n"
                "def stable_unique(strings):\n"
                "    return list(dict.fromkeys(strings))\n"
            )
            self.assertTrue(acceptance.oracle(answer)["passed"])

    def test_parser_handles_claude_list_and_codex_events(self):
        text = "\n".join(
            [
                json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Write"}]}}),
                json.dumps({"type": "result", "result": [{"type": "text", "text": '{"ok": true}'}], "usage": {"input_tokens": 10, "cache_read_input_tokens": 3}}),
                json.dumps({"type": "item.completed", "item": {"type": "agent_message", "text": '{"ok": true}'}}),
                json.dumps({"type": "item.completed", "item": {"type": "mcp_tool_call", "tool": "sqnic_read"}}),
                json.dumps({"type": "turn.completed", "usage": {"input_tokens": 20, "cache_creation_input_tokens": 4}}),
            ]
        )
        parsed = acceptance.parse_json_events(text)
        self.assertEqual(parsed["final"], '{"ok": true}')
        self.assertEqual(parsed["tool_calls"], ["Write", "sqnic_read"])
        self.assertEqual(parsed["usage"]["input_tokens"], 20)

    def test_parser_reports_failure_and_final_object_rejects_malformed_text(self):
        parsed = acceptance.parse_json_events(json.dumps({"type": "error", "message": "quota"}))
        self.assertEqual(parsed["failures"], ["quota"])
        self.assertIsNone(acceptance.parse_final_object("not json"))

    def test_usage_formulas_and_retrieval_failures(self):
        claude = acceptance.parse_json_events(
            json.dumps({"type": "result", "result": "ok", "usage": {"input_tokens": 10, "cache_creation_input_tokens": 4, "cache_read_input_tokens": 6}})
        )
        codex = acceptance.parse_json_events(
            json.dumps({"type": "turn.completed", "usage": {"input_tokens": 10, "cached_input_tokens": 6}})
        )
        self.assertEqual(claude["usage"]["input_tokens"] + claude["usage"]["cache_creation_input_tokens"] + claude["usage"]["cache_read_input_tokens"], 20)
        self.assertEqual(codex["usage"]["input_tokens"] - codex["usage"]["cached_input_tokens"], 4)
        failed = acceptance.retrieval_stats("codex", json.dumps({"type": "item.completed", "item": {"type": "command_execution", "command": "/bin/sqnic read-many", "exit_code": 1, "aggregated_output": "bad"}}), Path("/bin/sqnic"))
        self.assertEqual(failed["failed_command_count"], 1)
        self.assertEqual(failed["successful_retrieval_count"], 0)

    def test_compound_command_retains_retrieval_and_shell_failure(self):
        payload = json.dumps({"historical_data": True, "items": [{"status": "ok", "data": {"id": 1, "text": "original request"}}]})
        event = {"type": "item.completed", "item": {"type": "command_execution", "command": "/bin/sqnic read-many task 1 && rg missing", "exit_code": 1, "aggregated_output": "skill text\n" + payload + "\nno matches\n"}}
        text = json.dumps(event)
        stats = acceptance.retrieval_stats("codex", text, Path("/bin/sqnic"))
        self.assertEqual(stats["successful_retrieval_count"], 1)
        self.assertEqual(stats["failed_command_count"], 1)
        self.assertEqual(stats["retrieval_output_bytes"], len(payload.encode()))
        self.assertEqual(acceptance.first_retrieval_time_observed("codex", [(12.0, text.encode())], Path("/bin/sqnic"), 10.0), 2.0)

    def test_retrieval_rejects_empty_or_error_only_payload(self):
        for value in ({"items": []}, {"historical_data": True, "items": [{"status": "error"}]}):
            event = {"type": "item.completed", "item": {"type": "command_execution", "command": "/bin/sqnic read-many task 1", "exit_code": 0, "aggregated_output": json.dumps(value)}}
            self.assertEqual(acceptance.retrieval_stats("codex", json.dumps(event), Path("/bin/sqnic"))["successful_retrieval_count"], 0)

    def test_fixture_creates_nested_root_and_retrieves_history(self):
        binary = ROOT / "target" / "release" / "sqnic"
        if not binary.is_file():
            self.skipTest("release binary is not built")
        with tempfile.TemporaryDirectory() as directory:
            db, repo, _, _, context = acceptance.fixture(binary, Path(directory) / "nested" / "run")
            retrieved = acceptance.retrieval(binary, db, Path(repo))
            self.assertGreater(len(context), 0)
            self.assertGreater(retrieved["output_bytes"], 0)

    def test_buried_update_requires_retrieval_beyond_startup(self):
        binary = ROOT / "target" / "release" / "sqnic"
        if not binary.is_file():
            self.skipTest("release binary is not built")
        with tempfile.TemporaryDirectory() as directory:
            db, repo, _, update, context = acceptance.fixture(binary, Path(directory) / "run", buried=True)
            self.assertNotIn(update, context)
            self.assertTrue(json.loads(context.split("\n", 1)[1])["requests_omitted"])
            self.assertIn(update, acceptance.retrieval(binary, db, Path(repo))["output"])

    def test_only_real_sqnic_invocations_count(self):
        binary = Path("/bin/sqnic")
        self.assertTrue(acceptance.invokes_sqnic("/bin/zsh -lc '/bin/sqnic --db db read-many task 1'", binary))
        self.assertFalse(acceptance.invokes_sqnic("echo '/bin/sqnic read-many task 1'", binary))
        self.assertFalse(acceptance.invokes_sqnic("cat /bin/sqnic", binary))
        self.assertFalse(acceptance.invokes_sqnic("printf '%s' /bin/sqnic read-many", binary))

    def test_saved_release_gate_rejects_stale_partial_and_failed_evidence(self):
        report = {"source_sha256": acceptance.source_digest(), "live": True, "buried_update": True, "direct_spec_control": False, "repeats": 3, "binaries": {}}
        for index in range(3):
            runs = []
            for name, model in acceptance.MODEL.items():
                runs.append({"harness": name, "model": model, "exit_code": 0, "failed": False, "timed_out": False, "first_pass_correct": True, "parent_preflight": {"task": f"task-{index}-{name}"}, "required_evidence_complete": True, "measurement_complete": True, "retrieval": {"successful_retrieval_count": 1}, "oracle": {"passed": True, "cases": [{"ok": True} for _ in acceptance.expected_cases()]}})
            report["binaries"][f"candidate-run-{index + 1}"] = {"runs": runs}
        self.assertEqual(acceptance.validate_report(report), [])
        for field, value in [("failed", True), ("first_pass_correct", False), ("measurement_complete", "false")]:
            invalid = json.loads(json.dumps(report))
            invalid["binaries"]["candidate-run-1"]["runs"][0][field] = value
            self.assertTrue(acceptance.validate_report(invalid))
        invalid = json.loads(json.dumps(report))
        invalid["binaries"]["baseline-run-3"] = invalid["binaries"].pop("candidate-run-3")
        self.assertTrue(acceptance.validate_report(invalid))
        invalid = json.loads(json.dumps(report))
        invalid["binaries"]["candidate-run-2"]["runs"][0]["parent_preflight"]["task"] = "task-0-claude"
        self.assertTrue(acceptance.validate_report(invalid))

        report["source_sha256"] = "stale"
        self.assertTrue(acceptance.validate_report(report))
        report["source_sha256"] = acceptance.source_digest()
        report["binaries"]["candidate-run-3"]["runs"][0]["oracle"]["cases"][0]["ok"] = False
        self.assertTrue(acceptance.validate_report(report))
        report["binaries"].pop("candidate-run-3")
        self.assertTrue(acceptance.validate_report(report))

    def test_latency_gate_rejects_partial_or_small_sample_runs(self):
        for args in (["--samples", "2"], ["--samples", "50", "--operations", "startup_hook"]):
            result = subprocess.run([sys.executable, str(ROOT / "scripts/benchmark_automatic.py"), "--gate", "--output", "/tmp/unused-gate.json", *args], capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("gate requires", result.stderr)

    def test_oracle_rejects_bool_for_integer_cents(self):
        with tempfile.TemporaryDirectory() as directory:
            answer = Path(directory) / "answer.py"
            answer.write_text("def quote_cents(weight, zone):\n    return True\ndef expired(age_seconds, ttl_seconds):\n    return 1\ndef stable_unique(strings):\n    return strings\n")
            self.assertFalse(acceptance.oracle(answer)["passed"])

    def test_result_accounting_rejects_failures_and_missing_retrieval(self):
        base = {"exit_code": 0, "timed_out": False, "failures": [], "oracle": {"passed": True}, "retrieval": {"successful_retrieval_count": 1}}
        self.assertFalse(acceptance.run_failed(base, False))
        failed = {**base, "failures": ["max turns reached"]}
        self.assertTrue(acceptance.run_failed(failed, False))
        self.assertTrue(acceptance.run_failed({**base, "retrieval": {"successful_retrieval_count": 0}}, False))
        self.assertFalse(acceptance.run_failed({**base, "retrieval": {"successful_retrieval_count": 0}}, True))

    def test_live_guard_rejects_paid_harness_without_opt_in(self):
        with self.assertRaises(ValueError):
            acceptance.validate_live(False, "codex")
        acceptance.validate_live(True, "codex")

    def test_cli_requires_live(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/handoff_acceptance.py"), "--binary", "/tmp/missing", "--output", "/tmp/report.json", "--harness", "claude"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--live", result.stderr)


if __name__ == "__main__":
    unittest.main()
