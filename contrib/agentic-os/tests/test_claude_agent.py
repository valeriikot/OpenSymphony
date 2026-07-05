import os
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import claude_agent  # noqa: E402


def fake_claude(script_body: str) -> str:
    """Writes an executable fake `claude` binary and returns its path."""
    handle = tempfile.NamedTemporaryFile(
        "w", suffix=".sh", prefix="fake-claude-", delete=False
    )
    handle.write("#!/bin/sh\n" + script_body)
    handle.close()
    os.chmod(handle.name, os.stat(handle.name).st_mode | stat.S_IXUSR)
    return handle.name


STREAM = r"""cat > /dev/null
echo '{"type":"system","subtype":"init","session_id":"sess-1","model":"claude-sonnet-5"}'
echo '{"type":"assistant","session_id":"sess-1","message":{"content":[{"type":"text","text":"Working on it."}],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":10}}}'
echo '{"type":"assistant","session_id":"sess-1","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}],"usage":{"input_tokens":50,"output_tokens":5,"cache_read_input_tokens":0}}}'
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":1200,"total_cost_usd":0.0421,"result":"All done.","session_id":"sess-1","usage":{"input_tokens":400,"output_tokens":80,"cache_read_input_tokens":10}}'
"""


class NormalizationTests(unittest.TestCase):
    def test_launch_command_pins_headless_flags(self):
        command = claude_agent.build_command(model="claude-sonnet-5", claude_bin="claude")
        self.assertEqual(command[0], "claude")
        self.assertIn("--print", command)
        self.assertIn("stream-json", command)
        self.assertIn("bypassPermissions", command)
        self.assertEqual(command[-2:], ["--model", "claude-sonnet-5"])

    def test_system_init_normalizes_with_qualified_type(self):
        event = claude_agent.normalize_event(
            {"type": "system", "subtype": "init", "session_id": "s1", "model": "m"}
        )
        self.assertEqual(event["type"], "system.init")
        self.assertEqual(event["session_id"], "s1")
        self.assertIn("session started", claude_agent.event_summary(event))

    def test_non_events_are_ignored(self):
        self.assertIsNone(claude_agent.normalize_event({"jsonrpc": "2.0", "id": 1}))
        self.assertIsNone(claude_agent.normalize_event("text"))

    def test_token_usage_reads_message_usage(self):
        usage = claude_agent.token_usage(
            {"message": {"usage": {"input_tokens": 10, "output_tokens": 2}}}
        )
        self.assertEqual(usage["total_tokens"], 12)


class RunTests(unittest.TestCase):
    def test_successful_session_collects_text_usage_and_cost(self):
        binary = fake_claude(STREAM)
        events = []
        result = claude_agent.run_claude(
            "do the thing",
            claude_bin=binary,
            timeout=10,
            on_event=lambda event, summary: events.append(summary),
        )

        self.assertTrue(result.ok)
        self.assertEqual(result.text, "All done.")
        self.assertEqual(result.session_id, "sess-1")
        self.assertEqual(result.num_turns, 3)
        self.assertAlmostEqual(result.total_cost_usd, 0.0421)
        # Per-message usage accumulates (150/25/10), then the cumulative
        # result usage wins field-wise (400/80/10).
        self.assertEqual(result.usage["input_tokens"], 400)
        self.assertEqual(result.usage["output_tokens"], 80)
        self.assertEqual(result.usage["cache_read_tokens"], 10)
        self.assertIn("Claude: Working on it.", events)
        self.assertIn("Claude invoked tool Bash", events)

    def test_error_result_maps_to_failed(self):
        binary = fake_claude(
            'cat > /dev/null\n'
            'echo \'{"type":"result","subtype":"error_during_execution",'
            '"is_error":true,"session_id":"sess-2"}\'\n'
        )
        result = claude_agent.run_claude("x", claude_bin=binary, timeout=10)
        self.assertEqual(result.outcome, "failed")
        self.assertIn("error_during_execution", result.error)

    def test_stdout_closing_without_result_is_an_error(self):
        binary = fake_claude('cat > /dev/null\necho oops >&2\n')
        result = claude_agent.run_claude("x", claude_bin=binary, timeout=10)
        self.assertEqual(result.outcome, "error")
        self.assertIn("before a result event", result.error)
        self.assertIn("oops", result.error)

    def test_idle_timeout_kills_the_session(self):
        binary = fake_claude("cat > /dev/null\nsleep 30\n")
        result = claude_agent.run_claude("x", claude_bin=binary, timeout=0.5)
        self.assertEqual(result.outcome, "timeout")

    def test_missing_binary_reports_not_installed(self):
        result = claude_agent.run_claude(
            "x", claude_bin="/nonexistent/claude-bin", timeout=1
        )
        self.assertEqual(result.outcome, "not_installed")


if __name__ == "__main__":
    unittest.main()
