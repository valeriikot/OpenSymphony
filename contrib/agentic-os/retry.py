"""Retry helper with the backoff discipline from OpenSymphony's tracker clients.

Rules encoded here (each learned from a shipped bug):
    - Exponential backoff with a hard cap; growth never overflows.
    - A server-supplied Retry-After is honored only for rate limiting (429)
      and only up to ``retry_after_cap`` — transient 5xx responses can carry
      hour-long reset headers that must not put the caller to sleep.
    - Only transient failures retry; permanent ones surface immediately.

Composes with a circuit breaker: backoff absorbs transient noise, the breaker
handles sustained outage.
"""
from __future__ import annotations

import time


class TransientError(Exception):
    """A retryable failure. ``retry_after`` (seconds) is honored only when
    ``rate_limited`` is True, mirroring HTTP 429 semantics."""

    def __init__(
        self,
        message: str,
        retry_after: float | None = None,
        rate_limited: bool = False,
    ) -> None:
        super().__init__(message)
        self.retry_after = retry_after
        self.rate_limited = rate_limited


def classify_http_status(status: int) -> str:
    """'transient' | 'rate_limited' | 'permanent' for an HTTP status code."""
    if status == 429:
        return "rate_limited"
    if 500 <= status <= 599:
        return "transient"
    return "permanent"


def retry_call(
    fn,
    attempts: int = 3,
    initial_backoff: float = 0.25,
    max_backoff: float = 2.0,
    retry_after_cap: float = 30.0,
    sleep=time.sleep,
):
    """Calls ``fn()`` up to ``attempts`` times total.

    ``fn`` signals a retryable failure by raising TransientError; any other
    exception is permanent and propagates immediately. The final
    TransientError also propagates once the budget is exhausted.
    """
    if attempts < 1:
        raise ValueError("attempts must be >= 1")
    backoff = initial_backoff
    for attempt in range(1, attempts + 1):
        try:
            return fn()
        except TransientError as error:
            if attempt == attempts:
                raise
            delay = backoff
            if error.retry_after is not None:
                if error.rate_limited:
                    if error.retry_after > retry_after_cap:
                        # Waiting longer than the cap inline would stall the
                        # caller; surface instead of sleeping.
                        raise
                    delay = error.retry_after
                else:
                    # Never trust Retry-After on non-429 errors beyond our
                    # own ceiling.
                    delay = min(error.retry_after, max_backoff)
            sleep(delay)
            backoff = min(backoff * 2, max_backoff)
