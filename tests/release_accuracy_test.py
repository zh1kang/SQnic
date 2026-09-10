from __future__ import annotations

import collections
import json
import os
import sqlite3
import subprocess
import tempfile
import time
import unittest
from contextlib import closing
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BINARY = ROOT / "target" / "release" / "sqnic"
PROCESS_TIMEOUT = 120
REAP_TIMEOUT = 10


def configured_binary() -> Path:
    configured = os.environ.get("SQNIC_TEST_BINARY")
    path = Path(configured) if configured else DEFAULT_BINARY
    path = path.expanduser().resolve()
    if not path.is_file():
        if configured:
            raise RuntimeError(f"SQNIC_TEST_BINARY does not exist: {path}")
        raise unittest.SkipTest(f"release binary is unavailable: {path}")
    return path


class ReleaseAccuracyTest(unittest.TestCase):
    """Model-free release gates for the local database and CLI boundaries."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = configured_binary()

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="sqnic-release-accuracy-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.db = self.root / "context.sqlite3"
        self.children: list[subprocess.Popen[str]] = []
        self._git("init", "-q")
        self._git("config", "user.name", "SQnic release fixture")
        self._git("config", "user.email", "sqnic-release-fixture@example.invalid")

    def tearDown(self) -> None:
        for process in self.children:
            if process.poll() is None:
                process.kill()
                try:
                    process.wait(timeout=REAP_TIMEOUT)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=REAP_TIMEOUT)
            try:
                process.communicate(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=REAP_TIMEOUT)
                process.communicate(timeout=1)
        self.temporary.cleanup()

    def _command(self, *args: str) -> list[str]:
        return [str(self.binary), "--db", str(self.db), *args]

    def _run(self, *args: str, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment.update(
            {
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_TERMINAL_PROMPT": "0",
            }
        )
        return subprocess.run(
            self._command(*args),
            cwd=self.repo,
            input=input_text,
            capture_output=True,
            text=True,
            env=environment,
            timeout=PROCESS_TIMEOUT,
            check=False,
        )

    def _json(self, *args: str, input_text: str | None = None) -> dict:
        result = self._run(*args, input_text=input_text)
        self.assertEqual(
            result.returncode,
            0,
            f"{args!r}\nstdout={result.stdout}\nstderr={result.stderr}",
        )
        try:
            value = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            self.fail(f"{args!r} returned invalid JSON: {error}: {result.stdout!r}")
        self.assertIsInstance(value, dict)
        return value

    def _git(self, *args: str) -> str:
        environment = os.environ.copy()
        environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_TERMINAL_PROMPT": "0"})
        result = subprocess.run(
            ["git", "-C", str(self.repo), *args],
            capture_output=True,
            text=True,
            env=environment,
            timeout=30,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def _assert_failed(self, result: subprocess.CompletedProcess[str], text: str) -> None:
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(text.lower(), (result.stdout + result.stderr).lower())

    def _create(self, task: str) -> None:
        self._json("create", task, "--repo", str(self.repo))

    def _write_records(self, name: str, count: int, prefix: str) -> tuple[Path, collections.Counter[str]]:
        path = self.root / name
        expected: collections.Counter[str] = collections.Counter()
        with path.open("w", encoding="utf-8", newline="") as stream:
            for index in range(count):
                raw = json.dumps(
                    {
                        "type": "user",
                        "record": f"{prefix}-{index:06d}",
                        "text": f"payload-{prefix}-{index:06d}",
                    },
                    separators=(",", ":"),
                )
                line = f"{raw}\n"
                stream.write(line)
                expected[line] += 1
        return path, expected

    def _raw_counts(self, task: str) -> collections.Counter[str]:
        with closing(sqlite3.connect(self.db)) as connection, connection:
            rows = connection.execute(
                "SELECT raw, count(*) FROM events WHERE task=? GROUP BY raw", (task,)
            )
            return collections.Counter({raw: count for raw, count in rows})

    def _assert_integrity(self) -> None:
        with closing(sqlite3.connect(self.db)) as connection, connection:
            result = connection.execute("PRAGMA integrity_check").fetchone()
        self.assertEqual(result, ("ok",))

    def _communicate(self, process: subprocess.Popen[str]) -> subprocess.CompletedProcess[str]:
        try:
            stdout, stderr = process.communicate(timeout=PROCESS_TIMEOUT)
        except subprocess.TimeoutExpired:
            process.kill()
            try:
                process.wait(timeout=REAP_TIMEOUT)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=REAP_TIMEOUT)
            stdout, stderr = process.communicate(timeout=1)
            self.fail(
                f"SQnic subprocess exceeded {PROCESS_TIMEOUT}s: {stderr or stdout}"
            )
        finally:
            if process in self.children:
                self.children.remove(process)
        return subprocess.CompletedProcess(
            process.args, process.returncode, stdout, stderr
        )

    def _start_import(self, task: str, path: Path) -> subprocess.Popen[str]:
        environment = os.environ.copy()
        environment.update(
            {
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_TERMINAL_PROMPT": "0",
            }
        )
        process = subprocess.Popen(
            self._command("import", task, str(path)),
            cwd=self.repo,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=environment,
        )
        self.children.append(process)
        return process

    def _unpause_without_worker(self) -> None:
        enabled = self._json("unpause", "--repo", str(self.repo))
        with closing(sqlite3.connect(self.db)) as connection, connection:
            connection.execute(
                """
                INSERT INTO auto_leases(repo, token, expires)
                VALUES (?, 'release-test-controlled', ?)
                ON CONFLICT(repo) DO UPDATE SET token=excluded.token, expires=excluded.expires
                """,
                (enabled["repo"], int(time.time()) + 3600),
            )

    def _hook(self, session: str, transcript: Path) -> dict:
        payload = {
            "cwd": str(self.repo),
            "session_id": session,
            "transcript_path": str(transcript),
            "hook_event_name": "SessionStart",
        }
        result = self._run(
            "hook",
            "--repo",
            str(self.repo),
            "--harness",
            "claude",
            input_text=f"{json.dumps(payload)}\n",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_concurrent_50k_importers_retry_without_duplicate_raw_records(self) -> None:
        self._create("concurrent")
        path, expected = self._write_records("shared.jsonl", 50_000, "shared")

        processes = [
            self._start_import("concurrent", path),
            self._start_import("concurrent", path),
        ]
        results = [self._communicate(process) for process in processes]
        allowed_conflicts = ("database is locked", "source changed during import; retry")
        for result in results:
            if result.returncode != 0:
                message = (result.stdout + result.stderr).lower()
                self.assertTrue(
                    any(conflict in message for conflict in allowed_conflicts),
                    f"unexpected concurrent import failure: {result.stdout}{result.stderr}",
                )
        for _ in range(2):
            retry = self._run("import", "concurrent", str(path))
            self.assertEqual(retry.returncode, 0, retry.stderr)

        self.assertEqual(self._json("stats", "concurrent")["events"], 50_000)
        self.assertEqual(self._raw_counts("concurrent"), expected)
        self._assert_integrity()

    def test_killed_importer_rolls_back_and_retry_preserves_every_raw_record(self) -> None:
        self._create("crash-recovery")
        path, expected = self._write_records("recovery.jsonl", 120_000, "recovery")
        process = self._start_import("crash-recovery", path)
        observed_busy = False
        deadline = time.monotonic() + 45
        while process.poll() is None and time.monotonic() < deadline:
            if self.db.exists() and self.db.stat().st_size > 0:
                try:
                    with closing(sqlite3.connect(self.db, timeout=0)) as connection, connection:
                        connection.execute("BEGIN IMMEDIATE")
                        connection.rollback()
                except sqlite3.OperationalError as error:
                    if "locked" in str(error).lower():
                        observed_busy = True
                        break
            time.sleep(0.005)

        if not observed_busy:
            result = self._communicate(process)
            self.fail(
                "did not observe a real SQLite write lock before importer exited: "
                f"rc={result.returncode}, stderr={result.stderr}"
            )

        process.kill()
        killed = self._communicate(process)
        self.assertNotEqual(killed.returncode, 0)
        rows_after_kill = self._json("stats", "crash-recovery")["events"]
        self.assertIn(rows_after_kill, (0, len(expected)))
        if rows_after_kill == len(expected):
            self.assertEqual(self._raw_counts("crash-recovery"), expected)

        retry = self._run("import", "crash-recovery", str(path))
        self.assertEqual(retry.returncode, 0, retry.stderr)
        self.assertEqual(self._raw_counts("crash-recovery"), expected)
        self._assert_integrity()

    def test_explicit_auto_bindings_keep_same_repo_tasks_isolated(self) -> None:
        self._create("task-a")
        self._create("task-b")
        self._unpause_without_worker()
        first = self.repo / "session-a.jsonl"
        second = self.repo / "session-b.jsonl"
        first.write_text(
            json.dumps(
                {
                    "type": "user",
                    "sessionId": "session-a",
                    "cwd": str(self.repo),
                    "message": {"role": "user", "content": "task-a-private-marker"},
                }
            )
            + "\n",
            encoding="utf-8",
        )
        second.write_text(
            json.dumps(
                {
                    "type": "user",
                    "sessionId": "session-b",
                    "cwd": str(self.repo),
                    "message": {"role": "user", "content": "task-b-private-marker"},
                }
            )
            + "\n",
            encoding="utf-8",
        )
        self._hook("session-a", first)
        self._hook("session-b", second)
        for task, session in (("task-a", "session-a"), ("task-b", "session-b")):
            restored = self._json(
                "restore",
                "--repo",
                str(self.repo),
                "--task",
                task,
                "--harness",
                "claude",
                "--session",
                session,
            )
            self.assertEqual(restored["task"], task)

        self.assertEqual(
            len(self._json("search", "task-a", "task-a-private-marker")["matches"]), 1
        )
        self.assertEqual(
            len(self._json("search", "task-b", "task-b-private-marker")["matches"]), 1
        )
        self.assertEqual(
            self._json("search", "task-a", "task-b-private-marker")["matches"], []
        )
        self.assertEqual(
            self._json("search", "task-b", "task-a-private-marker")["matches"], []
        )

    def test_commit_queries_and_links_reject_foreign_task_events(self) -> None:
        self._create("commit-a")
        self._create("commit-b")
        tracked = self.repo / "tracked.txt"
        tracked.write_text("release evidence\n", encoding="utf-8")
        self._git("add", "tracked.txt")
        self._git(
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "release evidence commit",
        )
        commit_hash = self._git("rev-parse", "HEAD").strip()
        event = self._json(
            "capture",
            "commit-a",
            "requirement",
            "--record",
            json.dumps({"type": "user", "text": "commit-a requirement"}),
        )["event"]
        self._json("git-sync", "commit-a")
        commits = self._json("commits", "commit-a")["commits"]
        self.assertEqual(commits[0]["hash"], commit_hash)
        commit = self._json("commit", "commit-a", commit_hash)
        self.assertEqual(commit["metadata"]["hash"], commit_hash)
        read_many = self._json("read-many", "commit-a", f"commit:{commit_hash}")
        self.assertEqual(read_many["items"][0]["status"], "ok")
        self.assertEqual(read_many["items"][0]["data"]["metadata"]["hash"], commit_hash)

        foreign_event = self._run("read-many", "commit-b", str(event))
        self.assertEqual(foreign_event.returncode, 0)
        self.assertEqual(json.loads(foreign_event.stdout)["items"][0]["status"], "error")
        self._assert_failed(
            self._run(
                "link",
                "commit-b",
                str(event),
                commit_hash,
                "--relation",
                "tests",
                "--author",
                "release-fixture",
            ),
            "event not found in this task",
        )
        self._assert_failed(
            self._run("commit", "commit-b", commit_hash),
            "not indexed in this task",
        )
        foreign_commit = self._json(
            "read-many", "commit-b", f"commit:{commit_hash}"
        )
        self.assertEqual(foreign_commit["items"][0]["status"], "error")

    def test_detached_head_restore_is_repeatable_and_branch_switch_is_rejected(self) -> None:
        tracked = self.repo / "tracked.txt"
        tracked.write_text("detached evidence\n", encoding="utf-8")
        self._git("add", "tracked.txt")
        self._git(
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "detached baseline",
        )
        first_hash = self._git("rev-parse", "HEAD").strip()
        tracked.write_text("detached second evidence\n", encoding="utf-8")
        self._git("add", "tracked.txt")
        self._git(
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "detached second",
        )
        self._create("detached")
        self._unpause_without_worker()
        self._git("checkout", "--detach", first_hash)
        hook = self._hook("detached-session", self.root / "missing-transcript.jsonl")
        context = json.loads(
            hook["hookSpecificOutput"]["additionalContext"].split("\n", 1)[1]
        )
        self.assertEqual(context["branch"], f"detached:{first_hash}")
        self.assertTrue(Path(context["repo"]).samefile(self.repo))
        for _ in range(2):
            restored = self._json(
                "restore",
                "--repo",
                str(self.repo),
                "--task",
                "detached",
                "--harness",
                "claude",
                "--session",
                "detached-session",
            )
            self.assertEqual(restored["task"], "detached")
        with closing(sqlite3.connect(self.db)) as connection, connection:
            bindings = connection.execute(
                "SELECT count(*) FROM auto_sessions WHERE repo=? AND native_id=?",
                (context["repo"], "detached-session"),
            ).fetchone()[0]
        self.assertEqual(bindings, 1)
        self._git("checkout", "-qb", "switched")
        switched = self._run(
            "restore",
            "--repo",
            str(self.repo),
            "--task",
            "detached",
            "--harness",
            "claude",
            "--session",
            "detached-session",
        )
        self._assert_failed(switched, "branch")

    def test_same_size_replacement_is_visible_and_original_event_remains(self) -> None:
        self._create("replacement")
        original = json.dumps(
            {"type": "user", "record": "fixed-000000", "text": "original"},
            separators=(",", ":"),
        )
        replacement = json.dumps(
            {"type": "user", "record": "fixed-000000", "text": "replaced"},
            separators=(",", ":"),
        )
        self.assertEqual(len(original), len(replacement))
        path = self.root / "replacement.jsonl"
        path.write_text(f"{original}\n", encoding="utf-8", newline="")
        self._json("import", "replacement", str(path))
        replacement_path = self.root / "replacement.new.jsonl"
        replacement_path.write_text(f"{replacement}\n", encoding="utf-8", newline="")
        os.replace(replacement_path, path)
        failed = self._run("import", "replacement", str(path))
        self._assert_failed(failed, "prefix changed")
        self.assertEqual(self._json("read", "replacement", "1")["text"], f"{original}\n")
        self.assertEqual(self._json("stats", "replacement")["events"], 1)
        self._assert_integrity()


if __name__ == "__main__":
    unittest.main()
