import json
import os
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import notify  # noqa: E402


class CaptureHandler(BaseHTTPRequestHandler):
    captured = []

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        type(self).captured.append(
            (self.path, self.headers.get("Authorization"), json.loads(body))
        )
        status = 403 if self.path == "/fail" else 200
        self.send_response(status)
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, *args):
        pass


class NotifyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = HTTPServer(("127.0.0.1", 0), CaptureHandler)
        cls.base = f"http://127.0.0.1:{cls.server.server_port}"
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        CaptureHandler.captured = []
        for name in (
            notify.SLACK_WEBHOOK_URL_ENV,
            notify.LINE_CHANNEL_ACCESS_TOKEN_ENV,
            notify.LINE_RECIPIENT_ID_ENV,
        ):
            os.environ.pop(name, None)

    def test_disabled_without_configuration(self):
        self.assertFalse(notify.is_enabled())
        self.assertEqual(notify.notify_success("anything"), [])
        self.assertEqual(CaptureHandler.captured, [])

    def test_line_requires_both_variables(self):
        os.environ[notify.LINE_CHANNEL_ACCESS_TOKEN_ENV] = "token"
        self.assertFalse(notify.is_enabled())

    def test_completion_message_layout(self):
        message = notify.completion_message(
            "Skill 'code-review' completed", "Agent: claude · Run ab12", "https://x.test"
        )
        self.assertEqual(
            message,
            "✅ Skill 'code-review' completed\nAgent: claude · Run ab12\nhttps://x.test",
        )

    def test_line_payload_truncates_to_limit(self):
        payload = notify.line_payload("U1", "あ" * (notify.LINE_TEXT_LIMIT_CHARS + 5))
        self.assertEqual(
            len(payload["messages"][0]["text"]), notify.LINE_TEXT_LIMIT_CHARS
        )

    def test_delivers_to_both_channels_with_expected_shapes(self):
        os.environ[notify.SLACK_WEBHOOK_URL_ENV] = f"{self.base}/slack"
        os.environ[notify.LINE_CHANNEL_ACCESS_TOKEN_ENV] = "line-token"
        os.environ[notify.LINE_RECIPIENT_ID_ENV] = "U777"

        errors = notify.notify_success(
            "COE-42 implemented successfully",
            detail="Harness: claude_code",
            line_endpoint=f"{self.base}/line",
        )

        self.assertEqual(errors, [])
        by_path = {path: (auth, body) for path, auth, body in CaptureHandler.captured}
        self.assertIn("COE-42", by_path["/slack"][1]["text"])
        self.assertIsNone(by_path["/slack"][0])
        self.assertEqual(by_path["/line"][0], "Bearer line-token")
        self.assertEqual(by_path["/line"][1]["to"], "U777")
        self.assertIn("COE-42", by_path["/line"][1]["messages"][0]["text"])

    def test_failures_are_reported_not_raised(self):
        os.environ[notify.SLACK_WEBHOOK_URL_ENV] = f"{self.base}/fail"
        errors = notify.notify_success("boom")
        self.assertEqual(len(errors), 1)
        self.assertIn("slack", errors[0])
        self.assertIn("403", errors[0])


if __name__ == "__main__":
    unittest.main()
