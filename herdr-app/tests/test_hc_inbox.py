"""Unit tests for herdr-app/bin/hc-inbox that need no herdr-core.

Run: python3 -m unittest herdr-app/tests/test_hc_inbox.py
"""

import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.path.join(HERE, "..", "bin", "hc-inbox")
loader = importlib.machinery.SourceFileLoader("hc_inbox", BIN)
hci = importlib.util.module_from_spec(importlib.util.spec_from_loader("hc_inbox", loader))
loader.exec_module(hci)


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        p = unittest.mock.patch.multiple(hci, STATE=self.tmp.name, MODE_FILE=os.path.join(self.tmp.name, "mode"))
        p.start()
        self.addCleanup(p.stop)

    def fake_waiter(self, address, pid=None, claude_pid=0):
        d = hci.term_dir(address)
        os.makedirs(os.path.join(d, "queue"), exist_ok=True)
        with open(os.path.join(d, "waiter.json"), "w") as f:
            json.dump({"pid": pid or os.getpid(), "claude_pid": claude_pid, "since": 0}, f)


class EnvelopeTests(unittest.TestCase):
    def test_from_and_name(self):
        self.assertEqual(hci.envelope("hi", "t3c", "tc-ss", ""), '<hc from="t3c tc-ss">hi</hc>')

    def test_reach_and_escaping(self):
        e = hci.envelope("x", 't1', 'a"b', "hc send t1 when done or stuck")
        self.assertEqual(e, '<hc from="t1 a&quot;b" reach="hc send t1 when done or stuck">x</hc>')

    def test_no_sender(self):
        self.assertEqual(hci.envelope("x", "", "", ""), "<hc>x</hc>")


class PutTests(Base):
    def test_no_waiter_refuses(self):
        self.assertFalse(hci.put("t9", "<hc>x</hc>"))

    def test_put_queues_and_logs(self):
        # our own process counts as a live waiter only if its cmdline mentions hc-inbox
        with unittest.mock.patch.object(hci, "proc_cmdline", return_value=[b"python3", b"hc-inbox", b"wait"]):
            self.fake_waiter("t9")
            self.assertTrue(hci.put("t9", "<hc>one</hc>"))
            self.assertTrue(hci.put("t9", "<hc>two</hc>"))
        self.assertEqual(hci.claim(hci.term_dir("t9")), ["<hc>one</hc>", "<hc>two</hc>"])
        self.assertEqual(hci.claim(hci.term_dir("t9")), [])
        log = open(os.path.join(hci.term_dir("t9"), "log.jsonl")).read().splitlines()
        self.assertEqual(len(log), 2)

    def test_reused_pid_is_not_a_waiter(self):
        with unittest.mock.patch.object(hci, "proc_cmdline", return_value=[b"vim"]):
            self.fake_waiter("t9")
            self.assertFalse(hci.put("t9", "<hc>x</hc>"))

    def test_waiter_of_other_claude_refused(self):
        with unittest.mock.patch.object(hci, "proc_cmdline", return_value=[b"hc-inbox"]), \
                unittest.mock.patch.object(hci, "ancestors", return_value=[1]):
            self.fake_waiter("t9", claude_pid=111)
            self.assertFalse(hci.put("t9", "<hc>x</hc>", foreground_pid=222))
            self.assertTrue(hci.put("t9", "<hc>x</hc>", foreground_pid=111))

    def test_bad_address(self):
        with self.assertRaises(ValueError):
            hci.term_dir("../x")


class SendRoutingTests(Base):
    def run_send(self, argv, target):
        calls = []
        with unittest.mock.patch.object(hci, "hc_json", side_effect=lambda *a: target if a[1] != "me" else {}), \
                unittest.mock.patch.object(hci, "fallthrough", side_effect=lambda a: calls.append(a) or (_ for _ in ()).throw(SystemExit(0))), \
                unittest.mock.patch.object(hci, "put", return_value=True) as put, \
                unittest.mock.patch.dict(os.environ, {"HC_TERMINAL_ID": "me"}), \
                unittest.mock.patch("builtins.print"):
            try:
                hci.cmd_send(argv)
            except SystemExit:
                pass
        return calls, put

    CLAUDE = {"address": "t5", "state": "live", "status": {"app": "claude-code"}, "foreground": {"name": "claude", "pid": 9}, "parent": "me"}

    def test_options_fall_through(self):
        calls, put = self.run_send(["--wait", "t5", "x"], self.CLAUDE)
        self.assertEqual(calls, [["--wait", "t5", "x"]])
        put.assert_not_called()

    def test_full_mode_falls_through(self):
        with unittest.mock.patch("builtins.print"):
            hci.cmd_mode(["full"])
        calls, put = self.run_send(["t5", "x"], self.CLAUDE)
        self.assertTrue(calls)
        put.assert_not_called()

    def test_non_claude_falls_through(self):
        calls, _ = self.run_send(["t5", "x"], {"address": "t5", "state": "live", "status": {"app": "pi"}, "foreground": {"name": "pi"}})
        self.assertTrue(calls)

    def test_claude_goes_to_inbox_with_reach(self):
        calls, put = self.run_send(["t5", "hello"], self.CLAUDE)
        self.assertEqual(calls, [])
        addr, env = put.call_args.args[:2]
        self.assertEqual(addr, "t5")
        self.assertEqual(env, '<hc from="me" reach="hc send me when done or stuck">hello</hc>')


class LogTests(Base):
    def test_parse_envelope(self):
        self.assertEqual(hci.parse_envelope('<hc from="t3c tc-ss" reach="hc send t3c when done">a\nb</hc>'),
                         {"from": "t3c", "from_name": "tc-ss", "body": "a\nb"})
        self.assertEqual(hci.parse_envelope('<hc from="dev/t1">x &lt; y</hc>')["from"], "dev/t1")
        self.assertEqual(hci.parse_envelope("plain")["body"], "plain")

    def test_log_json_all_since(self):
        os.makedirs(hci.term_dir("t1"))
        os.makedirs(hci.term_dir("t2"))
        with unittest.mock.patch.object(hci.time, "time", return_value=100):
            hci.append_log("t1", '<hc from="t9">old</hc>')
        with unittest.mock.patch.object(hci.time, "time", return_value=200):
            hci.append_log("t2", '<hc from="t9 boss">new</hc>')
        out = []
        with unittest.mock.patch("builtins.print", side_effect=out.append):
            hci.cmd_log(["--json", "--all", "--since", "150"])
        doc = json.loads(out[0])
        self.assertEqual(doc["mode"], "compact")
        self.assertEqual(doc["messages"], [{"to": "t2", "ts": 200, "from": "t9", "from_name": "boss", "body": "new"}])


class HookConfigTests(Base):
    def test_merge_is_idempotent_and_keeps_others(self):
        s = {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}, "x": 1}
        hci.merge_hooks(s, "/p/hc-inbox")
        hci.merge_hooks(s, "/p/hc-inbox")
        cmds = [h["command"] for g in s["hooks"]["Stop"] for h in g["hooks"]]
        self.assertEqual(cmds, ["other", "/p/hc-inbox wait"])
        self.assertEqual(len(s["hooks"]["SessionStart"]), 1)
        self.assertTrue(s["hooks"]["Stop"][1]["hooks"][0]["asyncRewake"])


class WaitTests(Base):
    def test_wait_delivers_and_exits_2(self):
        """Run the real `wait` as a subprocess (it registers itself), queue a message, expect exit 2."""
        env = dict(os.environ, HC_TERMINAL_ID="t7", XDG_STATE_HOME=os.path.dirname(self.tmp.name),
                   HC_INBOX_LIFETIME_S="20")
        state = os.path.join(os.path.dirname(self.tmp.name), "hc-inbox")
        p = subprocess.Popen([sys.executable, BIN, "wait"], env=env, stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        p.stdin.write("{}")
        p.stdin.close()
        wfile = os.path.join(state, "t7", "waiter.json")
        for _ in range(50):
            if os.path.exists(wfile):
                break
            time.sleep(0.1)
        r = subprocess.run([sys.executable, BIN, "put", "t7"], env=env, input="<hc>ping</hc>", text=True)
        self.assertEqual(r.returncode, 0)
        p.wait(timeout=10)
        err = p.stderr.read()
        p.stdout.close()
        p.stderr.close()
        self.assertEqual(p.returncode, 2)
        self.assertIn("<hc>ping</hc>", err)
        self.assertFalse(os.path.exists(wfile))
        r = subprocess.run([sys.executable, BIN, "put", "t7"], env=env, input="x", text=True)
        self.assertEqual(r.returncode, 3)  # nobody waiting now
        import shutil
        shutil.rmtree(os.path.join(state, "t7"), ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
