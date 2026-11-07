"""Unit tests for herdr-app/bin/cards that need no herdr-core.

Run: python3 -m unittest herdr-app/tests/test_cards.py
"""

import argparse
import contextlib
import copy
import importlib.machinery
import importlib.util
import io
import json
import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
loader = importlib.machinery.SourceFileLoader("cards", os.path.join(HERE, "..", "bin", "cards"))
cards = importlib.util.module_from_spec(importlib.util.spec_from_loader("cards", loader))
loader.exec_module(cards)

TASK = "---\nid: SHE-1\nstatus: in-progress\n---\n\n## Description\n\nx\n\n## Decisions\n\n- old one\n\n## Validation\n\ny\n"


class WritebackTests(unittest.TestCase):
    def test_status_replaced_only_in_front_matter(self):
        text = cards.set_status(TASK + "\nstatus: body text\n", "verify")
        self.assertIn("status: verify\n---", text)
        self.assertIn("status: body text", text)

    def test_status_added_when_missing(self):
        self.assertIn("status: done", cards.set_status("---\nid: X-1\n---\nbody\n", "done"))

    def test_decisions_appended_once_under_their_section(self):
        once = cards.add_decisions(TASK, ["old one", "new choice"])
        self.assertEqual(once, cards.add_decisions(once, ["old one", "new choice"]))
        decisions = once.split("## Decisions")[1].split("## Validation")[0]
        self.assertEqual(decisions.count("- old one"), 1)
        self.assertIn("- new choice", decisions)

    def test_decisions_section_created_when_missing(self):
        self.assertTrue(cards.add_decisions("# Title\n", ["a"]).endswith("## Decisions\n\n- a\n"))

    def test_writeback_end_to_end_keeps_mode(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "sheprd"))
            path = os.path.join(root, "sheprd", "SHE-1.md")
            with open(path, "w") as f:
                f.write(TASK)
            os.chmod(path, 0o644)
            cards.writeback_config = lambda: {
                "markdown_path": "{root}/{project}/{id}.md", "root": root,
                "projects": {"SHE": "sheprd"}, "done_status": "verify",
            }
            card = {"id": "SHE-1", "decisions": [{"text": "x"}], "summary": "Merged", "done_at": "2026-11-01T00:00:00+00:00"}
            self.assertEqual(cards.writeback(card), {"ok": True, "message": "wrote SHE-1.md"})
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o644)
            with open(path) as f:
                self.assertIn("- Done 2026-11-01: Merged", f.read())
            self.assertFalse(cards.writeback({**card, "id": "ZZ-1"})["ok"])


class AnswerTests(unittest.TestCase):
    def test_plain_answers_stay_bare_and_others_are_quoted(self):
        self.assertEqual(cards.answer_text("Stripe"), "Stripe")
        self.assertEqual(cards.answer_text('EUR "only"'), '"EUR \\"only\\""')


class ImportTests(unittest.TestCase):
    def test_sidebar_toml_import_drops_sheprd_marks(self):
        with tempfile.TemporaryDirectory() as home:
            os.makedirs(os.path.join(home, ".config", "herdr"))
            with open(os.path.join(home, ".config", "herdr", "sidebar.toml"), "w") as f:
                f.write('hidden = ["dev/x"]\nunread = ["dev/w1:p1"]\n[[group]]\nname = "A"\nmatch = ["a"]\n')
            old = os.environ["HOME"]
            os.environ["HOME"] = home
            try:
                view = cards.sidebar_import()
            finally:
                os.environ["HOME"] = old
        self.assertEqual(view["projects"], [{"name": "A", "match": ["a"]}])
        self.assertEqual(view["hidden"], ["dev/x"])
        self.assertEqual(view["marks"]["unread"], [])


class FakeCore:
    """herdr-core's doc.get/doc.set in memory: versions, conflicts, the 1 MiB cap."""

    docs = {}
    cap = 1024 * 1024

    def call(self, method, params):
        name = params["name"]
        value, version = self.docs.get(name, (None, 0))
        if method == "doc.get":
            return {"value": copy.deepcopy(value), "version": version}
        if params["version"] != version:
            raise cards.RpcError({"message": "conflict", "data": {"kind": "conflict"}})
        if len(json.dumps(params["value"])) > self.cap:
            raise cards.RpcError({"message": "document exceeds 1 MiB of JSON", "data": {"kind": "invalid_params"}})
        self.docs[name] = (copy.deepcopy(params["value"]), version + 1)
        return {"version": version + 1}

    def close(self):
        pass


def ns(**kw):
    base = {"terminal": "t7", "title": None, "status": None, "summary": None, "link": None}
    return argparse.Namespace(**{**base, **kw})


def quiet(fn, *args):
    with contextlib.redirect_stdout(io.StringIO()):
        fn(*args)


class LedgerTests(unittest.TestCase):
    def setUp(self):
        FakeCore.docs = {}
        FakeCore.cap = 1024 * 1024
        self.saved = (cards.Core, cards.born_of, subprocess.run)
        cards.Core = FakeCore
        self.born = {"t7": 1}
        cards.born_of = lambda t: self.born.get(t)
        self.sent = []
        self.during_send = None

        def run(argv, **kw):
            self.sent.append(argv[-1])
            if self.during_send:
                hook, self.during_send = self.during_send, None
                hook()
            return subprocess.CompletedProcess(argv, 0, "", "")

        subprocess.run = run

    def tearDown(self):
        cards.Core, cards.born_of, subprocess.run = self.saved

    def doc(self):
        return cards.read_doc()

    def ask(self, card_id, question):
        quiet(cards.cmd_ask, ns(id=card_id, question=question, option=None, recommend=None))

    def stage(self, n, answer, card_id="SHE-1"):
        quiet(cards.cmd_stage, argparse.Namespace(terminal="t7", id=card_id, n=n, answer=answer))

    def test_send_marks_sent_only_what_it_typed(self):
        self.ask("SHE-1", "Provider?")
        self.ask("SHE-1", "Currency?")
        self.stage(1, "A")

        def meanwhile():  # a click while hc send waits for the person to pause
            self.stage(2, "late answer")
            with self.assertRaises(SystemExit):  # Q1 is being sent: it can't change under the send
                with contextlib.redirect_stderr(io.StringIO()):
                    self.stage(1, "changed mind")

        self.during_send = meanwhile
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        self.assertEqual(self.sent, ["SHE-1 Q1 (Provider?) → A"])
        q1, q2 = self.doc()["cards"]["t7/SHE-1"]["questions"]
        self.assertEqual((q1["state"], q1["answer"]), ("sent", "A"))
        self.assertEqual((q2["state"], q2["answer"]), ("staged", "late answer"))
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        self.assertEqual(self.sent[-1], "SHE-1 Q2 (Currency?) → late answer")

    def test_send_puts_dashes_after_double_dash(self):
        self.ask("SHE-1", "Provider?")
        self.stage(1, "-h")
        argv = []
        subprocess.run = lambda a, **kw: argv.extend(a) or subprocess.CompletedProcess(a, 0, "", "")
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        self.assertEqual(argv[1:4], ["send", "--", "t7"])

    def test_a_claim_left_by_a_dead_send_expires(self):
        self.ask("SHE-1", "Provider?")
        self.stage(1, "A")
        value, version = FakeCore.docs[cards.DOC]
        q = value["cards"]["t7/SHE-1"]["questions"][0]
        q.update(state="sending", claim="dead", claimed_at=cards.now())
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            self.stage(1, "B")  # fresh claim: refused
        q["claimed_at"] = "2000-01-01T00:00:00+00:00"
        self.stage(1, "B")  # stale claim: may be staged again
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        self.assertEqual(self.sent, ["SHE-1 Q1 (Provider?) → B"])
        sent = self.doc()["cards"]["t7/SHE-1"]["questions"][0]
        self.assertEqual(sent["state"], "sent")
        self.assertNotIn("claim", sent)

    def test_second_send_finds_nothing(self):
        self.ask("SHE-1", "Provider?")
        self.stage(1, "A")
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cards.cmd_send(argparse.Namespace(terminal="t7"))
        self.assertEqual(len(self.sent), 1)

    def test_failed_send_puts_answers_back(self):
        self.ask("SHE-1", "Provider?")
        self.stage(1, "A")
        subprocess.run = lambda argv, **kw: subprocess.CompletedProcess(argv, 1, "", "blocked")
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cards.cmd_send(argparse.Namespace(terminal="t7"))
        self.assertEqual(self.doc()["cards"]["t7/SHE-1"]["questions"][0]["state"], "staged")
        self.assertEqual(self.doc()["inbox"], {})

    def backdate(self, key, days):
        doc = self.doc()
        doc["cards"][key]["updated_at"] = (cards.dt.datetime.now(cards.dt.timezone.utc) - cards.dt.timedelta(days=days)).isoformat(timespec="seconds")
        FakeCore.docs[cards.DOC] = (doc, FakeCore.docs[cards.DOC][1])

    def test_a_new_program_in_the_terminal_keeps_open_cards(self):
        self.ask("SHE-1", "Provider?")
        self.born["t7"] = 2  # the terminal got a new program (core restart or new session)
        quiet(cards.cmd_update, ns(id="SHE-2", status="dev"))
        card = self.doc()["cards"]["t7/SHE-1"]
        self.assertEqual(len(card["questions"]), 1)
        self.assertEqual(card["born"], 2)  # adopted by the new program

    def test_a_new_program_drops_finished_cards(self):
        self.ask("SHE-1", "Provider?")
        quiet(cards.cmd_done, ns(id="SHE-1", summary="ok"))
        self.born["t7"] = 2
        quiet(cards.cmd_update, ns(id="SHE-2", status="dev"))
        self.assertNotIn("t7/SHE-1", self.doc()["cards"])

    def test_a_new_program_lets_stale_open_cards_expire(self):
        self.ask("SHE-1", "Provider?")
        self.backdate("t7/SHE-1", cards.KEEP_PREVIOUS_DAYS + 1)
        self.born["t7"] = 2
        quiet(cards.cmd_update, ns(id="SHE-2", status="dev"))
        self.assertNotIn("t7/SHE-1", self.doc()["cards"])

    def test_a_card_the_program_keeps_updating_never_expires_on_restart_alone(self):
        self.ask("SHE-1", "Provider?")
        self.backdate("t7/SHE-1", cards.KEEP_PREVIOUS_DAYS - 1)
        self.born["t7"] = 2
        quiet(cards.cmd_update, ns(id="SHE-2", status="dev"))
        self.assertIn("t7/SHE-1", self.doc()["cards"])

    def test_answers_for_the_previous_program_are_not_delivered_to_the_next(self):
        self.ask("SHE-1", "Provider?")
        self.stage(1, "A")
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        self.assertTrue(self.doc()["inbox"]["t7"])  # unread, typed for the first program
        self.born["t7"] = 2  # a new program in t7: maybe another task
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cards.cmd_answers(argparse.Namespace(terminal="t7", peek=False))
        self.assertEqual(out.getvalue().strip(), "no new answers")
        self.assertEqual(self.doc()["inbox"].get("t7", []), [])
        # the answer is still on its question, on the adopted card
        question = self.doc()["cards"]["t7/SHE-1"]["questions"][0]
        self.assertEqual((question["state"], question["answer"]), ("sent", "A"))

    def test_an_inferred_update_never_overrides_an_explicit_one(self):
        quiet(cards.cmd_update, ns(id="SHE-1", title="Mine", status="dev"))
        quiet(cards.cmd_update, ns(id="SHE-1", title="Guess", status="running", summary="from a hook", inferred=True))
        card = self.doc()["cards"]["t7/SHE-1"]
        self.assertEqual((card["title"], card["status"], card["summary"]), ("Mine", "dev", "from a hook"))

    def test_an_inferred_update_does_not_reopen_a_done_card(self):
        quiet(cards.cmd_update, ns(id="SHE-1", title="Mine"))
        quiet(cards.cmd_done, ns(id="SHE-1", summary="ok"))
        quiet(cards.cmd_update, ns(id="SHE-1", status="running", inferred=True))
        card = self.doc()["cards"]["t7/SHE-1"]
        self.assertTrue(card["done"])
        self.assertEqual(card["status"], "done")

    def test_inferred_questions_are_replaced_and_can_be_cleared(self):
        quiet(cards.cmd_update, ns(id="session", inferred=True, inferred_question=["Monthly?"]))
        quiet(cards.cmd_update, ns(id="session", inferred=True, inferred_question=["Yearly?"]))
        texts = [q["text"] for q in self.doc()["cards"]["t7/session"]["questions"] if q["state"] == "open"]
        self.assertEqual(texts, ["Yearly?"])
        quiet(cards.cmd_update, ns(id="session", inferred=True, clear_questions=True))
        self.assertEqual([q for q in self.doc()["cards"]["t7/session"]["questions"] if q["state"] == "open"], [])

    def test_an_explicit_question_survives_inferred_replacement(self):
        self.ask("SHE-1", "Provider?")
        quiet(cards.cmd_update, ns(id="SHE-1", inferred=True, clear_questions=True))
        self.assertEqual(len(self.doc()["cards"]["t7/SHE-1"]["questions"]), 1)

    def test_write_over_the_cap_prunes_history_and_lands(self):
        for i in range(30):
            self.ask(f"SHE-{i}", "q")
            quiet(cards.cmd_done, ns(id=f"SHE-{i}", summary="x" * 500))
        FakeCore.cap = len(json.dumps(self.doc())) - 100
        quiet(cards.cmd_update, ns(id="SHE-99", status="dev"))
        self.assertIn("t7/SHE-99", self.doc()["cards"])
        self.assertFalse(any(c["done"] for c in self.doc()["cards"].values()))

    def test_prune_bounds_answered_questions_and_read_answers(self):
        doc = cards.empty_doc()
        card = cards.get_card(doc, "t7", "SHE-1", create=True)
        card["questions"] = [{"n": i, "state": "sent"} for i in range(50)] + [{"n": 99, "state": "open"}]
        doc["inbox"]["t7"] = [{"line": str(i), "at": cards.now(), "read": True} for i in range(40)]
        cards.prune(doc)
        self.assertEqual(len(card["questions"]), 21)
        self.assertEqual(card["questions"][-1]["n"], 99)
        self.assertEqual(len(doc["inbox"]["t7"]), 5)


class WritebackEdgeTests(unittest.TestCase):
    def test_empty_front_matter_gets_status_inside_it(self):
        self.assertEqual(cards.set_status("---\n---\nbody\n\n---\n", "done"), "---\nstatus: done\n---\nbody\n\n---\n")

    def test_decisions_heading_inside_a_code_fence_is_ignored(self):
        text = "# T\n\n```md\n## Decisions\n```\n\n## Decisions\n\n- a\n"
        out = cards.add_decisions(text, ["b"])
        self.assertIn("```md\n## Decisions\n```", out)
        self.assertTrue(out.endswith("- a\n- b\n"))

    def test_crlf_and_bom_survive(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "p"))
            path = os.path.join(root, "p", "SHE-2.md")
            with open(path, "w", newline="") as f:
                f.write("﻿" + TASK.replace("SHE-1", "SHE-2").replace("\n", "\r\n"))
            cards.writeback_config = lambda: {"markdown_path": "{root}/{project}/{id}.md", "root": root, "projects": {"SHE": "p"}}
            card = {"id": "SHE-2", "decisions": [{"text": "x"}], "summary": None, "done_at": "2026-11-01T00:00:00+00:00"}
            self.assertTrue(cards.writeback(card)["ok"])
            with open(path, "rb") as f:
                raw = f.read()
        self.assertTrue(raw.startswith(b"\xef\xbb\xbf---\r\n"))
        self.assertNotIn(b"\n", raw.replace(b"\r\n", b""))
        self.assertIn(b"status: done\r\n", raw)
        self.assertIn(b"- x\r\n", raw)


if __name__ == "__main__":
    unittest.main()
