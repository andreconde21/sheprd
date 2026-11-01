import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
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

    def test_an_older_herdr_pauses_the_ledger(self):
        self.fake_herdr('echo "error: unknown command: card" >&2; exit 2')
        tasks = {"SHE-1": ["--title", "one", "--status", "running"],
                 "SHE-2": ["--title", "two", "--status", "running"]}
        hook.send_ledger("s1", "w1:p1", tasks, None)
        hook.send_ledger("s1", "w1:p1", tasks, None)
        self.assertEqual(len(self.calls()), 1)
        self.assertTrue(hook.load_ledger_state("s1")["unsupported"])

    def test_a_usage_error_does_not_stop_the_ledger(self):
        self.fake_herdr('echo "missing value for --title" >&2; exit 2')
        tasks = {"SHE-1": ["--title", "one", "--status", "running"]}
        hook.send_ledger("s1", "w1:p1", tasks, None)
        hook.send_ledger("s1", "w1:p1", tasks, None)
        self.assertEqual(len(self.calls()), 2)
        self.assertNotIn("unsupported", hook.load_ledger_state("s1"))

    def test_an_unsupported_server_is_tried_again_after_an_hour(self):
        self.fake_herdr("exit 0")
        hook.save_ledger_state("s1", {"unsupported": time.time() - hook.LEDGER_REPROBE - 1})
        tasks = {"SHE-1": ["--title", "one", "--status", "running"]}
        hook.send_ledger("s1", "w1:p1", tasks, None)
        self.assertEqual(len(self.calls()), 1)

    def test_ids_that_read_as_flags_are_not_cards(self):
        self.assertIsNone(hook.LEDGER_ID.match("-x"))
        self.assertIsNone(hook.LEDGER_ID.match("--pane"))
        self.assertIsNotNone(hook.LEDGER_ID.match("SHE-1"))
        self.assertEqual(hook.task_updates([{"key": "--pane", "label": "x", "stages": []}]), {})

    def test_a_slow_older_batch_never_lands_after_a_newer_one(self):
        # The first status update takes a while; the next turn's batch starts meanwhile.
        self.fake_herdr('case "$*" in *running*) sleep 1;; esac; exit 0')
        script = HOOK_PATH.resolve()
        env = {**os.environ}

        def batch(status, seq):
            payload = json.dumps({"tasks": {"SHE-1": ["--title", "one", "--status", status]},
                                  "asked": None, "seq": seq})
            child = subprocess.Popen([sys.executable, str(script), "--ledger", "s1", "w1:p1"],
                                     stdin=subprocess.PIPE, env=env, text=True)
            child.stdin.write(payload)
            child.stdin.close()
            return child

        older = batch("running", 1)
        time.sleep(0.2)
        newer = batch("done", 2)
        older.wait(10)
        newer.wait(10)
        statuses = [call.split("--status ")[1] for call in self.calls()]
        self.assertEqual(statuses[-1], "done")
        self.assertEqual(hook.load_ledger_state("s1")["sent"]["SHE-1"][-1], "done")

        # A batch that is older than the last one sent is dropped.
        late = batch("running", 1)
        late.wait(10)
        self.assertEqual(hook.load_ledger_state("s1")["sent"]["SHE-1"][-1], "done")
        self.assertEqual(len(self.calls()), len(statuses))

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



class SummaryQuestionTests(unittest.TestCase):
    """Questions from the summarizer and the pane-text fallback (SHE-100009 review): no
    placeholders, no shell prompts."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.cache = mock.patch.object(hook, "CACHE_DIR", self.tmp.name)
        self.cache.start()
        self.report = mock.patch.object(hook, "report")
        self.report.start()
        self.ledger = mock.patch.object(hook, "send_ledger")
        self.sent = self.ledger.start()

    def tearDown(self):
        self.ledger.stop()
        self.report.stop()
        self.cache.stop()
        self.tmp.cleanup()

    def asked(self, command, text):
        with mock.patch.object(hook, "summary_command", return_value=command):
            hook.summarize("s1", text, "w1:p1", len(text), from_pane=True)
        return self.sent.call_args.args[3] if self.sent.called else []

    def test_the_prompt_has_no_example_values_to_echo(self):
        self.assertNotIn('"..."', hook.SUMMARY_PROMPT)
        self.assertNotIn("{", hook.SUMMARY_PROMPT)

    def test_placeholder_questions_from_the_summarizer_are_dropped(self):
        reply = json.dumps({"title": "t", "questions": ["...", "?", "…?", "Deploy now?"]})
        script = Path(self.tmp.name) / "reply.json"
        script.write_text(reply)
        self.assertEqual(self.asked(f"cat {script}", "some pane text"), ["Deploy now?"])

    def test_the_pane_fallback_keeps_only_prose_questions(self):
        for line in ("$ ls foo?", "> really?", "# why?", "% which?", "ok?", "a b ?"):
            self.assertFalse(hook.prose_question(line), line)
        self.assertTrue(hook.prose_question("Should I deploy it now?"))
        text = "building\nShould I deploy it now?\n$ git status?\n"
        self.assertEqual(self.asked(None, text), ["Should I deploy it now?"])
        self.sent.reset_mock()
        self.assertEqual(self.asked(None, "build\nrun\n$ git status?\n"), [])


class ModelTokenTests(unittest.TestCase):
    """sheprd_model: the model of the agent's last real reply (SHE-100011)."""

    def reply(self, msg_id, model):
        return json.dumps({
            "type": "assistant", "timestamp": "2026-10-30T10:00:00Z",
            "message": {"id": msg_id, "model": model, "content": [],
                        "usage": {"input_tokens": 10, "output_tokens": 5}},
        }) + "\n"

    def read(self, *lines):
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as handle:
            handle.writelines(lines)
        self.addCleanup(os.unlink, handle.name)
        cache = hook.fresh_cache()
        hook.read_new_lines(cache, handle.name, [])
        return hook.tokens(cache)

    def test_the_last_reply_names_the_model(self):
        tokens = self.read(self.reply("a", "claude-sonnet-4-6"), self.reply("b", "claude-opus-5-5"))
        self.assertEqual(tokens["sheprd_model"], "claude-opus-5-5")

    def test_synthetic_messages_do_not_replace_the_model(self):
        tokens = self.read(self.reply("a", "claude-opus-5-5"), self.reply("b", "<synthetic>"))
        self.assertEqual(tokens["sheprd_model"], "claude-opus-5-5")

    def test_no_reply_yet_reports_no_model(self):
        self.assertNotIn("sheprd_model", self.read())


if __name__ == "__main__":
    unittest.main()
