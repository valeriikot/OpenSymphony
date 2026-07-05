import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from retry import TransientError, classify_http_status, retry_call  # noqa: E402


class RetryTests(unittest.TestCase):
    def setUp(self):
        self.sleeps = []

    def sleep(self, seconds):
        self.sleeps.append(seconds)

    def test_succeeds_after_transient_failures_with_doubling_backoff(self):
        calls = {"n": 0}

        def flaky():
            calls["n"] += 1
            if calls["n"] < 3:
                raise TransientError("blip")
            return "ok"

        result = retry_call(flaky, attempts=3, sleep=self.sleep)
        self.assertEqual(result, "ok")
        self.assertEqual(self.sleeps, [0.25, 0.5])

    def test_backoff_is_capped(self):
        def always():
            raise TransientError("blip")

        with self.assertRaises(TransientError):
            retry_call(always, attempts=5, max_backoff=0.6, sleep=self.sleep)
        self.assertEqual(self.sleeps, [0.25, 0.5, 0.6, 0.6])

    def test_permanent_errors_do_not_retry(self):
        calls = {"n": 0}

        def broken():
            calls["n"] += 1
            raise ValueError("permanent")

        with self.assertRaises(ValueError):
            retry_call(broken, attempts=3, sleep=self.sleep)
        self.assertEqual(calls["n"], 1)
        self.assertEqual(self.sleeps, [])

    def test_retry_after_on_non_rate_limited_errors_is_clamped(self):
        calls = {"n": 0}

        def with_huge_reset_header():
            calls["n"] += 1
            if calls["n"] == 1:
                # A 503 carrying Retry-After: 3600 must not sleep an hour.
                raise TransientError("503", retry_after=3600, rate_limited=False)
            return "ok"

        result = retry_call(with_huge_reset_header, attempts=2, sleep=self.sleep)
        self.assertEqual(result, "ok")
        self.assertEqual(self.sleeps, [2.0])  # clamped to max_backoff

    def test_rate_limited_retry_after_is_honored_within_cap(self):
        calls = {"n": 0}

        def rate_limited():
            calls["n"] += 1
            if calls["n"] == 1:
                raise TransientError("429", retry_after=1.5, rate_limited=True)
            return "ok"

        result = retry_call(rate_limited, attempts=2, sleep=self.sleep)
        self.assertEqual(result, "ok")
        self.assertEqual(self.sleeps, [1.5])

    def test_rate_limited_beyond_cap_surfaces_instead_of_sleeping(self):
        def rate_limited():
            raise TransientError("429", retry_after=120, rate_limited=True)

        with self.assertRaises(TransientError):
            retry_call(rate_limited, attempts=3, retry_after_cap=30, sleep=self.sleep)
        self.assertEqual(self.sleeps, [])

    def test_http_status_classification(self):
        self.assertEqual(classify_http_status(429), "rate_limited")
        self.assertEqual(classify_http_status(503), "transient")
        self.assertEqual(classify_http_status(404), "permanent")
        self.assertEqual(classify_http_status(401), "permanent")


if __name__ == "__main__":
    unittest.main()
