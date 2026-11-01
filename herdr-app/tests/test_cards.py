"""Unit tests for herdr-app/bin/cards that need no herdr-core.

Run: python3 -m unittest herdr-app/tests/test_cards.py
"""

import importlib.machinery
import importlib.util
import os
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
            self.assertIn("- Done 2026-11-01: Merged", open(path).read())
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


if __name__ == "__main__":
    unittest.main()
