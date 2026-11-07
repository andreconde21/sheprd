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
import shutil
import subprocess
import tempfile
import time
import unittest
import unittest.mock

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

    def test_the_agent_closes_a_question_the_user_answered_in_chat(self):
        self.ask("SHE-1", "Provider?")
        self.ask("SHE-1", "Currency?")
        quiet(cards.cmd_answered, ns(id="SHE-1", question=1, all=False, answer="Stripe"))
        q1, q2 = self.doc()["cards"]["t7/SHE-1"]["questions"]
        self.assertEqual((q1["state"], q1["answer"]), ("answered", "Stripe"))
        self.assertIn("answered_at", q1)
        self.assertEqual(q2["state"], "open")
        self.assertFalse(cards.live(q1))

    def test_answered_without_an_answer_text_keeps_none(self):
        self.ask("SHE-1", "Provider?")
        quiet(cards.cmd_answered, ns(id="SHE-1", question=1, all=False, answer=None))
        self.assertIsNone(self.doc()["cards"]["t7/SHE-1"]["questions"][0]["answer"])

    def test_answered_all_closes_every_open_question_but_not_sent_ones(self):
        self.ask("SHE-1", "Provider?")
        self.ask("SHE-1", "Currency?")
        self.stage(1, "A")
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))  # Q1 is now sent
        quiet(cards.cmd_answered, ns(id="SHE-1", question=None, all=True, answer=None))
        states = [q["state"] for q in self.doc()["cards"]["t7/SHE-1"]["questions"]]
        self.assertEqual(states, ["sent", "answered"])

    def test_answered_needs_a_live_question_and_a_target(self):
        self.ask("SHE-1", "Provider?")
        quiet(cards.cmd_answered, ns(id="SHE-1", question=1, all=False, answer=None))
        for args in (ns(id="SHE-1", question=1, all=False, answer=None),  # already closed
                     ns(id="SHE-1", question=9, all=False, answer=None),
                     ns(id="SHE-1", question=None, all=False, answer=None),  # no target
                     ns(id="NOPE-1", question=1, all=False, answer=None)):
            with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
                cards.cmd_answered(args)

    def replied(self, *items):
        return quiet(cards.cmd_replied, ns(items=list(items)))

    def states(self):
        return [q["state"] for q in self.doc()["cards"]["t7/SHE-1"]["questions"]]

    def test_replied_marks_only_open_questions(self):
        for text in ("A?", "B?", "C?", "D?"):
            self.ask("SHE-1", text)
        self.stage(2, "x")
        quiet(cards.cmd_answered, ns(id="SHE-1", question=3, all=False, answer=None))
        quiet(cards.cmd_dismiss, ns(terminal="t7", id="SHE-1", question=4))
        self.replied("SHE-1:1", "SHE-1:2", "SHE-1:3", "SHE-1:4", "SHE-1:9", "NOPE-1:1", "bad", "SHE-1:x")
        self.assertEqual(self.states(), ["replied", "staged", "answered", "dismissed"])
        self.assertIn("replied_at", self.doc()["cards"]["t7/SHE-1"]["questions"][0])
        self.assertFalse(cards.live(self.doc()["cards"]["t7/SHE-1"]["questions"][0]))

    def test_a_replied_question_can_still_be_answered_or_reopened(self):
        self.ask("SHE-1", "A?")
        self.ask("SHE-1", "B?")
        self.replied("SHE-1:1", "SHE-1:2")
        quiet(cards.cmd_answered, ns(id="SHE-1", question=1, all=False, answer="Stripe"))
        quiet(cards.cmd_reopen, ns(terminal="t7", id="SHE-1", question=2))
        self.assertEqual(self.states(), ["answered", "open"])
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            cards.cmd_reopen(ns(terminal="t7", id="SHE-1", question=1))  # answered, not replied

    def test_a_replied_question_is_dismissed_like_any_other(self):
        self.ask("SHE-1", "A?")
        self.replied("SHE-1:1")
        quiet(cards.cmd_dismiss, ns(terminal="t7", id="SHE-1", question=1))
        self.assertEqual(self.states(), ["dismissed"])

    def test_a_new_inferred_question_set_leaves_replied_ones_alone(self):
        quiet(cards.cmd_update, ns(id="SHE-1", title=None, status=None, summary=None, link=None, inferred=True,
                                   inferred_question=["Old?"], clear_questions=False))
        self.replied("SHE-1:1")
        quiet(cards.cmd_update, ns(id="SHE-1", title=None, status=None, summary=None, link=None, inferred=True,
                                   inferred_question=["New?"], clear_questions=False))
        self.assertEqual(self.states(), ["replied", "open"])

    def test_a_question_being_sent_is_not_closed_under_the_send(self):
        self.ask("SHE-1", "Provider?")
        self.stage(1, "A")

        def meanwhile():
            with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
                cards.cmd_answered(ns(id="SHE-1", question=1, all=False, answer=None))

        self.during_send = meanwhile
        quiet(cards.cmd_send, argparse.Namespace(terminal="t7"))
        self.assertEqual(self.doc()["cards"]["t7/SHE-1"]["questions"][0]["state"], "sent")

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


import struct


def png(width=1, height=1):
    return b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">II", width, height) + b"\x08\x02\0\0\0" + b"\0" * 8


def gif(width=1, height=1, frames=None):
    """A gif with a screen of width x height and the given frames (default: one filling it)."""
    body = b"".join(b"\x2c" + struct.pack("<HHHH", 0, 0, w, h) + b"\0\x02\x01\0\0" for w, h in (frames or [(width, height)]))
    return b"GIF89a" + struct.pack("<HH", width, height) + b"\0\0\0" + body + b"\x3b"


def jpeg(width=1, height=1):
    return (b"\xff\xd8\xff\xe0\x00\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0"
            + b"\xff\xc0\x00\x11\x08" + struct.pack(">HH", height, width) + b"\x03" + b"\0" * 9)


def webp_x(width=1, height=1):
    return (b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0" + (width - 1).to_bytes(3, "little")
            + (height - 1).to_bytes(3, "little"))


def webp_l(width=1, height=1):
    return b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f" + ((width - 1) | ((height - 1) << 14)).to_bytes(4, "little")


def webp_lossy(width=1, height=1):
    return (b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9d\x01\x2a" + struct.pack("<HH", width, height))


PNG = png()
WEBP = webp_x()


def evidence_args(**kw):
    base = dict(id="SHE-1", image=None, test=None, link=None, label=None, clear=False)
    return ns(**{**base, **kw})


class EvidenceTests(unittest.TestCase):
    setUp_ledger = LedgerTests.setUp
    tearDown_ledger = LedgerTests.tearDown
    doc = LedgerTests.doc

    def setUp(self):
        self.setUp_ledger()
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.store = os.path.join(self.tmp.name, "store")
        self.env = unittest.mock.patch.dict(os.environ, {"SHEPRD_EVIDENCE_DIR": self.store})
        self.env.start()

    def tearDown(self):
        self.env.stop()
        self.tearDown_ledger()

    def file(self, name, data):
        path = os.path.join(self.tmp.name, name)
        with open(path, "wb") as handle:
            handle.write(data)
        return path

    def add(self, **kw):
        quiet(cards.cmd_evidence, evidence_args(**kw))

    def items(self):
        return self.doc()["cards"]["t7/SHE-1"]["evidence"]

    def refuses(self, fn, args):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            fn(args)

    def test_images_are_copied_to_a_private_folder_and_read_back_as_base64(self):
        self.add(image=self.file("shot.png", PNG), label="Login page")
        item = self.items()[0]
        self.assertEqual((item["kind"], item["mime"], item["label"]), ("image", "image/png", "Login page"))
        stored = os.path.join(self.store, item["file"])
        with open(stored, "rb") as handle:
            self.assertEqual(handle.read(), PNG)
        self.assertEqual(os.stat(stored).st_mode & 0o777, 0o600)
        self.assertEqual(os.stat(self.store).st_mode & 0o777, 0o700)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cards.cmd_evidence_image(ns(id="SHE-1", n=1))
        import base64
        self.assertEqual(base64.b64decode(out.getvalue()), PNG)

    def test_webp_is_accepted_and_other_types_are_not(self):
        self.add(image=self.file("a.webp", WEBP))
        self.assertEqual(self.items()[0]["mime"], "image/webp")
        for name, data in (("a.svg", b"<svg xmlns='x'/>"), ("a.png", b"not a png at all"), ("a.txt", b"")):
            self.refuses(cards.cmd_evidence, evidence_args(image=self.file(name, data)))

    def test_image_sizes_are_read_from_every_format_and_capped(self):
        for make in (png, gif, jpeg, webp_x, webp_l, webp_lossy):
            self.assertEqual(cards.image_size(make(640, 480)), (640, 480), make.__name__)
            self.assertEqual(cards.image_size(make(4000, 4000)), (4000, 4000), make.__name__)
            self.add(image=self.file(f"ok-{make.__name__}", make(4000, 4000)))  # exactly 16 Mpx
            self.add(image=self.file(f"wide-{make.__name__}", make(16000, 1000)))  # long, but within the budget
            for wide, tall in ((4001, 4000), (8000, 8000), (16383, 16383)):
                self.refuses(cards.cmd_evidence, evidence_args(image=self.file("big", make(wide, tall))))

    def test_a_gif_is_checked_frame_by_frame(self):
        self.add(image=self.file("ok.gif", gif(100, 100, [(100, 100)] * 50)))
        # a small screen whose frames claim a huge area
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("f1.gif", gif(10, 10, [(10, 10), (8000, 8000)]))))
        # every frame within the budget, together far beyond it
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("f2.gif", gif(4000, 4000, [(4000, 4000)] * 5))))
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("f3.gif", gif(10, 10, [(10, 10)] * 501))))
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("f4.gif", gif(10, 10, [(0, 5)]))))
        # a gif whose blocks cannot be followed
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("f5.gif", gif(10, 10)[:-3])))
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("f6.gif", b"GIF89a" + struct.pack("<HH", 1, 1) + b"\0\0\0\x99")))

    def test_an_image_whose_size_cannot_be_read_is_refused(self):
        for data in (b"\x89PNG\r\n\x1a\n" + b"\0" * 4, b"\xff\xd8\xff\xe0\x00\x02", b"RIFF\0\0\0\0WEBPVP8Z" + b"\0" * 20):
            self.refuses(cards.cmd_evidence, evidence_args(image=self.file("odd", data)))

    def test_a_stored_file_swapped_for_an_oversized_image_is_not_served(self):
        self.add(image=self.file("shot.png", PNG))
        with open(os.path.join(self.store, self.items()[0]["file"]), "wb") as handle:
            handle.write(png(5000, 5000))
        self.refuses(cards.cmd_evidence_image, ns(id="SHE-1", n=1))

    def test_a_loose_evidence_folder_is_tightened_and_a_foreign_one_refused(self):
        os.makedirs(self.store, mode=0o755)
        os.chmod(self.store, 0o755)
        self.add(image=self.file("shot.png", PNG))
        self.assertEqual(os.stat(self.store).st_mode & 0o777, 0o700)
        link = os.path.join(self.tmp.name, "linked")
        os.symlink(self.store, link)
        with unittest.mock.patch.dict(os.environ, {"SHEPRD_EVIDENCE_DIR": link}):
            self.refuses(cards.cmd_evidence, evidence_args(image=self.file("shot2.png", png(2, 2))))

    def test_a_planted_temp_link_is_never_written_through(self):
        self.add(image=self.file("first.png", PNG))
        target = os.path.join(self.tmp.name, "victim.txt")
        with open(target, "w") as handle:
            handle.write("keep")
        data = png(3, 3)
        name = f"t7-SHE-1-{__import__('hashlib').sha256(data).hexdigest()[:16]}.png"
        os.symlink(target, os.path.join(self.store, f".{name}.{os.getpid()}.tmp"))
        self.add(image=self.file("second.png", data))
        with open(target) as handle:
            self.assertEqual(handle.read(), "keep")
        self.assertEqual(self.items()[-1]["file"], name)

    def test_only_regular_files_under_the_size_cap(self):
        fifo = os.path.join(self.tmp.name, "fifo.png")
        os.mkfifo(fifo)
        started = time.time()
        self.refuses(cards.cmd_evidence, evidence_args(image=fifo))
        self.assertLess(time.time() - started, 2)
        os.symlink("/dev/zero", os.path.join(self.tmp.name, "zero.png"))
        self.refuses(cards.cmd_evidence, evidence_args(image=os.path.join(self.tmp.name, "zero.png")))
        self.refuses(cards.cmd_evidence, evidence_args(image=self.tmp.name))
        self.refuses(cards.cmd_evidence, evidence_args(image=os.path.join(self.tmp.name, "missing.png")))
        self.refuses(cards.cmd_evidence, evidence_args(image=self.file("big.png", PNG + b"\0" * cards.IMAGE_MAX)))
        self.assertEqual(self.doc()["cards"].get("t7/SHE-1", {}).get("evidence", []), [])

    def test_a_stored_file_that_grew_or_changed_is_not_served(self):
        self.add(image=self.file("shot.png", PNG))
        stored = os.path.join(self.store, self.items()[0]["file"])
        with open(stored, "wb") as handle:
            handle.write(b"<html>")
        self.refuses(cards.cmd_evidence_image, ns(id="SHE-1", n=1))
        self.refuses(cards.cmd_evidence_image, ns(id="SHE-1", n=2))

    def test_test_runs_keep_passed_and_failed(self):
        self.add(test="unit: 37/0")
        self.add(test="e2e: 12 / 3")
        self.add(test="smoke ok")
        first, second, third = self.items()
        self.assertEqual((first["passed"], first["failed"]), (37, 0))
        self.assertEqual((second["passed"], second["failed"]), (12, 3))
        self.assertEqual((third["passed"], third["failed"]), (None, None))

    def test_links_must_be_http(self):
        self.add(link="https://example.test/deploy/1", label="Deploy check")
        self.add(link="https://example.test/x")
        self.assertEqual([i["label"] for i in self.items()], ["Deploy check", "example.test"])
        for bad in ("javascript:alert(1)", "file:///etc/passwd", "https://a b", "ftp://x.test/", "//x.test"):
            self.refuses(cards.cmd_evidence, evidence_args(link=bad))

    def test_exactly_one_kind_per_call(self):
        self.refuses(cards.cmd_evidence, evidence_args())
        self.refuses(cards.cmd_evidence, evidence_args(test="a: 1/0", link="https://x.test/"))
        self.refuses(cards.cmd_evidence, evidence_args(test="a: 1/0", clear=True))

    def test_only_the_newest_twelve_stay_and_dropped_images_leave_the_disk(self):
        self.add(image=self.file("first.png", PNG))
        first = self.items()[0]["file"]
        for n in range(12):
            self.add(test=f"run {n}: {n}/0")
        items = self.items()
        self.assertEqual(len(items), 12)
        self.assertEqual(items[0]["text"], "run 0: 0/0")
        self.assertFalse(os.path.exists(os.path.join(self.store, first)))

    def test_the_same_image_twice_survives_dropping_one_copy(self):
        path = self.file("shot.png", PNG)
        self.add(image=path)
        self.add(image=path)
        self.assertEqual(self.items()[0]["file"], self.items()[1]["file"])
        for n in range(11):
            self.add(test=f"t{n}: 1/0")
        self.assertEqual(len(self.items()), 12)
        self.assertTrue(os.path.exists(os.path.join(self.store, self.items()[0]["file"])))

    def test_clear_removes_items_and_files(self):
        self.add(image=self.file("shot.png", PNG))
        name = self.items()[0]["file"]
        self.add(clear=True)
        self.assertEqual(self.items(), [])
        self.assertFalse(os.path.exists(os.path.join(self.store, name)))

    def test_a_tampered_file_name_in_the_document_is_never_read_or_deleted(self):
        victim = self.file("victim", PNG)
        self.add(test="a: 1/0")

        def forge(doc):
            doc["cards"]["t7/SHE-1"]["evidence"] = [{"kind": "image", "file": "../victim", "mime": "image/png"}]

        cards.change(forge)
        self.refuses(cards.cmd_evidence_image, ns(id="SHE-1", n=1))
        for name in ("..", "."):
            cards.change(lambda doc, name=name: doc["cards"]["t7/SHE-1"].update(
                evidence=[{"kind": "image", "file": name, "mime": "image/png"}]))
            self.refuses(cards.cmd_evidence_image, ns(id="SHE-1", n=1))
        self.add(clear=True)
        self.assertTrue(os.path.exists(victim))


PANEL = os.path.join(HERE, "..", "mods", "sheprd-panel", "main.js")

# Hosts that point inside the machine or its network, in the forms a URL parser also accepts.
INTERNAL = [
    "http://localhost/", "http://LOCALHOST:8080/", "http://localhost./", "http://app.localhost/", "http://a.b.localhost./",
    "http://127.0.0.1/", "http://127.255.255.254/", "http://0x7f.1/", "http://0177.0.0.1/", "http://2130706433/",
    "http://127.1/", "http://0.0.0.0/", "http://0/",
    "http://[::1]/", "http://[::]/", "http://[0:0:0:0:0:0:0:1]/", "http://[::ffff:127.0.0.1]/", "http://[::ffff:7f00:1]/",
    "http://[::ffff:10.1.2.3]/", "http://[::127.0.0.1]/",
    "http://10.0.0.1/", "http://10.255.255.255/", "http://172.16.0.1/", "http://172.31.255.255/", "http://192.168.1.1/",
    "http://169.254.169.254/", "http://100.64.0.1/", "http://100.127.255.255/",
    "http://[fc00::1]/", "http://[fd12:3456::1]/", "http://[fe80::1]/", "http://[febf::1]/", "http://[fe80::1%25eth0]/",
]


@unittest.skipUnless(shutil.which("node"), "node is needed to run the panel's filter block")
class PanelFilterTests(unittest.TestCase):
    """The panel's status filter: its marked block of main.js, run in node."""

    def run_js(self, body):
        with open(PANEL) as handle:
            source = handle.read()
        block = source.split("// filter:begin", 1)[1].split("// filter:end", 1)[0].split("\n", 1)[1]
        script = ('const live = (q) => q.state === "open" || q.state === "staged" || q.state === "sending";\n'
                  + block + "\n" + body)
        result = subprocess.run(["node", "-e", script], capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_every_status_maps_to_one_chip_and_unknown_ones_only_show_under_all(self):
        out = self.run_js('''
          const card = (status, extra = {}) => ({ status, done: false, questions: [], ...extra });
          const names = ["needs", "working", "review", "blocked", "done", "all"];
          const where = (c) => names.filter((n) => cardMatches(c, n));
          console.log(JSON.stringify({
            "in-progress": where(card("in-progress")), working: where(card("Working")), running: where(card("running")),
            review: where(card("review")), prOpen: where(card("PR open")), verify: where(card("verify")),
            blocked: where(card("blocked")), stopped: where(card("stopped")),
            done: where(card("done")), toDeploy: where(card("to-deploy")),
            flagged: where(card("x", { done: true })),
            unknown: where(card("idle")), none: where(card(null)),
          }));
        ''')
        self.assertEqual(out["in-progress"], ["working", "all"])
        self.assertEqual(out["working"], ["working", "all"])
        self.assertEqual(out["running"], ["working", "all"])
        self.assertEqual((out["review"], out["prOpen"], out["verify"]), (["review", "all"],) * 3)
        self.assertEqual((out["blocked"], out["stopped"]), (["blocked", "all"],) * 2)
        self.assertEqual((out["done"], out["toDeploy"], out["flagged"]), (["done"],) * 3)
        self.assertEqual((out["unknown"], out["none"]), (["all"], ["all"]))

    def test_open_questions_need_you_whatever_the_status_and_keep_a_done_card_visible(self):
        out = self.run_js('''
          const q = (state) => ({ state });
          const cards = [
            { status: "working", done: false, questions: [q("open")] },
            { status: "done", done: true, questions: [q("open")] },
            { status: "review", done: false, questions: [q("answered"), q("sent"), q("dismissed")] },
            { status: "done", done: true, questions: [] },
            { status: "weird", done: false, questions: [] },
          ];
          console.log(JSON.stringify(filterCounts(cards)));
        ''')
        self.assertEqual(out, {"needs": 2, "working": 1, "review": 1, "blocked": 0, "done": 2, "all": 4})

    def run_url(self, urls):
        with open(PANEL) as handle:
            source = handle.read()
        block = source.split("// url:begin", 1)[1].split("// url:end", 1)[0].split("\n", 1)[1]
        script = block + f"\nconsole.log(JSON.stringify({json.dumps(urls)}.map((u) => safeUrl(u))));"
        result = subprocess.run(["node", "-e", script], capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_only_plain_http_urls_reach_xdg_open(self):
        good = ["https://example.test/a?b=1", "http://example.test:8080/x", "https://8.8.8.8/x", "https://[2606:4700::1111]/",
                "https://172.32.0.1/", "https://172.15.255.255/", "https://100.63.0.1/", "https://192.169.0.1/", "https://notlocalhost.test/"]
        bad = ["file:///home/me/x.desktop", "ssh://host/", "javascript:alert(1)", "data:text/html,x", "ftp://x.test/",
               "custom-handler://run?x", "-https://x.test", "--help", "//x.test", "https://", "https://a b.test",
               "https://x.test/\nsecond", "", "x" * 10, "https://example.test/" + "a" * 2100, "mailto:a@b.test", "vscode://file/etc"] + INTERNAL
        for url, out in zip(good + bad, self.run_url(good + bad)):
            if url in good:
                self.assertTrue(out["href"].startswith(url.split("://")[0] + "://"))
            else:
                self.assertIsNone(out, url)
        self.assertEqual(self.run_url(good)[0]["host"], "example.test")
        self.assertEqual(self.run_url(good)[1]["host"], "example.test:8080")

    def test_the_opened_url_is_the_parsed_one_not_the_raw_string(self):
        out = self.run_url(["HTTPS://Example.TEST:443/a b".replace(" ", "%20"), "https://example.test/x\\y", "http://user:pw@example.test/?q=\u00e9"])
        self.assertEqual(out[0]["href"], "https://example.test/a%20b")
        self.assertEqual(out[1]["href"], "https://example.test/x/y")
        self.assertEqual(out[2]["href"], "http://user:pw@example.test/?q=%C3%A9")

    def test_every_xdg_open_in_the_panel_goes_through_the_check(self):
        with open(PANEL) as handle:
            source = handle.read()
        self.assertEqual(source.count('exec("xdg-open"'), 1)
        self.assertIn('exec("xdg-open", [safe.href])', source)

    def test_the_chips_and_their_remembered_choice_are_wired_in_the_panel(self):
        with open(PANEL) as handle:
            source = handle.read()
        self.assertIn('stored("filter", "all")', source)
        self.assertIn('store("filter", key)', source)
        self.assertRegex(source, r"const stored = .*try \{ return localStorage")
        self.assertIn("FILTERS.some(([key]) => key === saved)", source)  # a bad saved value falls back


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
