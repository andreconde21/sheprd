import importlib.machinery
import importlib.util
import json
import os
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

HOOK_PATH = Path(__file__).with_name("sheprd-claude-hook")


def load_hook():
    loader = importlib.machinery.SourceFileLoader("sheprd_claude_hook", str(HOOK_PATH))
    spec = importlib.util.spec_from_loader("sheprd_claude_hook", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


hook = load_hook()


class CardLedgerTests(unittest.TestCase):
    """The hook's inferred card ledger writes (SHE-100008), against a fake herdr."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.log = self.root / "calls.log"
        self.env = mock.patch.dict(os.environ, {"HOME": str(self.root), "HERDR_WORKSPACE_ID": "w1"})
        self.env.start()
        self.cache = mock.patch.object(hook, "CACHE_DIR", str(self.root / ".cache/sheprd/claude-hook"))
        self.cache.start()

    def tearDown(self):
        self.cache.stop()
        self.env.stop()
        self.tmp.cleanup()

    def fake_herdr(self, body):
        script = self.root / "herdr"
        script.write_text(f'#!/bin/sh\necho "$@" >> "{self.log}"\n{body}\n')
        script.chmod(0o755)
        os.environ["HERDR_BIN_PATH"] = str(script)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_an_older_herdr_stops_the_ledger_for_the_session(self):
        self.fake_herdr('echo "error: unknown command: card" >&2; exit 2')
        tasks = {"SHE-1": ["--title", "one", "--status", "running"],
                 "SHE-2": ["--title", "two", "--status", "running"]}
        hook.send_ledger("s1", "w1:p1", tasks, None)
        hook.send_ledger("s1", "w1:p1", tasks, None)
        self.assertEqual(len(self.calls()), 1)
        self.assertTrue(hook.load_ledger_state("s1")["unsupported"])

    def test_sent_updates_are_not_resent(self):
        self.fake_herdr("exit 0")
        tasks = {"SHE-1": ["--title", "one", "--status", "running"]}
        hook.send_ledger("s1", "w1:p1", tasks, None)
        hook.send_ledger("s1", "w1:p1", tasks, None)
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(hook.ledger_plan(hook.load_ledger_state("s1"), tasks, None, "session"), {})

    def test_a_failed_workspace_lookup_is_tried_again(self):
        self.fake_herdr('case "$1" in workspace) exit 1;; esac; exit 0')
        hook.send_ledger("s1", "w1:p1", {}, ["Monthly?"])
        state = hook.load_ledger_state("s1")
        self.assertNotIn("card", state)
        self.assertIn("session", state["sent"])
        hook.send_ledger("s1", "w1:p1", {}, ["Monthly?"])
        lookups = [call for call in self.calls() if call.startswith("workspace")]
        self.assertEqual(len(lookups), 2)

    def test_a_found_workspace_card_is_remembered(self):
        reply = json.dumps({"result": {"workspace": {"tokens": {"card": "SHE-9"}}}})
        self.fake_herdr(f"case \"$1\" in workspace) echo '{reply}';; esac; exit 0")
        hook.send_ledger("s1", "w1:p1", {}, ["Monthly?"])
        hook.send_ledger("s1", "w1:p1", {}, ["Yearly?"])
        self.assertEqual(len([c for c in self.calls() if c.startswith("workspace")]), 1)
        self.assertEqual(hook.load_ledger_state("s1")["card"], "SHE-9")
        self.assertIn("SHE-9", hook.load_ledger_state("s1")["sent"])

    def test_ledger_text_is_one_line_without_control_characters(self):
        self.assertEqual(hook.one_line("red \x1b[31m\nnext\tline\x9b", 400), "red [31m next line")

    def test_the_hook_does_not_wait_for_the_server(self):
        self.fake_herdr("sleep 2; exit 0")
        tasks = {"SHE-1": ["--title", "one", "--status", "running"]}
        started = time.monotonic()
        hook.start_ledger("s1", "w1:p1", tasks, None)
        self.assertLess(time.monotonic() - started, 1.0)
        deadline = time.monotonic() + 10
        while "sent" not in hook.load_ledger_state("s1") and time.monotonic() < deadline:
            time.sleep(0.1)
        self.assertEqual(hook.load_ledger_state("s1")["sent"], tasks)


if __name__ == "__main__":
    unittest.main()
