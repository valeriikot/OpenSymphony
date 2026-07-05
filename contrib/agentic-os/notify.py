"""Outbound Slack/LINE notifications for Agentic OS.

Drop-in port of OpenSymphony's `opensymphony-notify` module. Stdlib only —
no new entries in requirements.txt.

Configuration (environment variables):
    SLACK_WEBHOOK_URL          Slack incoming webhook for the target channel.
    LINE_CHANNEL_ACCESS_TOKEN  LINE Messaging API channel access token.
    LINE_RECIPIENT_ID          LINE user/group/room id to push to.

Semantics (keep these when integrating):
    - Best-effort: delivery failures are returned as strings for logging and
      must never fail the skill run / cron job that triggered them.
    - A failing channel does not block the other.
    - Unconfigured channels are skipped; nothing configured => no requests.
    - LINE text is truncated to the API's 5000-character limit.
"""
from __future__ import annotations

import json
import os
import urllib.error
import urllib.request

SLACK_WEBHOOK_URL_ENV = "SLACK_WEBHOOK_URL"
LINE_CHANNEL_ACCESS_TOKEN_ENV = "LINE_CHANNEL_ACCESS_TOKEN"
LINE_RECIPIENT_ID_ENV = "LINE_RECIPIENT_ID"

LINE_PUSH_ENDPOINT = "https://api.line.me/v2/bot/message/push"

REQUEST_TIMEOUT_SECONDS = 10
LINE_TEXT_LIMIT_CHARS = 5000


def _env(name: str) -> str | None:
    value = os.environ.get(name, "").strip()
    return value or None


def is_enabled() -> bool:
    """True when at least one channel is fully configured."""
    return bool(_env(SLACK_WEBHOOK_URL_ENV)) or bool(
        _env(LINE_CHANNEL_ACCESS_TOKEN_ENV) and _env(LINE_RECIPIENT_ID_ENV)
    )


def completion_message(
    title: str,
    detail: str | None = None,
    url: str | None = None,
) -> str:
    """Human-readable message shared by all channels."""
    message = f"✅ {title.strip()}"
    if detail and detail.strip():
        message += f"\n{detail.strip()}"
    if url and url.strip():
        message += f"\n{url.strip()}"
    return message


def slack_payload(message: str) -> dict:
    return {"text": message}


def line_payload(recipient_id: str, message: str) -> dict:
    return {
        "to": recipient_id,
        "messages": [{"type": "text", "text": message[:LINE_TEXT_LIMIT_CHARS]}],
    }


def _post_json(url: str, payload: dict, bearer_token: str | None = None) -> str | None:
    """POSTs JSON; returns an error description or None on success."""
    body = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"}, method="POST"
    )
    if bearer_token:
        request.add_header("Authorization", f"Bearer {bearer_token}")
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT_SECONDS) as response:
            if 200 <= response.status < 300:
                return None
            return f"HTTP {response.status}"
    except urllib.error.HTTPError as error:
        detail = error.read(300).decode("utf-8", "replace")
        return f"HTTP {error.code}: {detail}"
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        return f"request failed: {error}"


def notify_success(
    title: str,
    detail: str | None = None,
    url: str | None = None,
    line_endpoint: str = LINE_PUSH_ENDPOINT,
) -> list[str]:
    """Sends the message to every configured channel.

    Returns a list of error descriptions (empty on full success) so callers
    can log without exceptions ever escaping into the triggering flow.
    """
    message = completion_message(title, detail, url)
    errors: list[str] = []

    slack_url = _env(SLACK_WEBHOOK_URL_ENV)
    if slack_url:
        error = _post_json(slack_url, slack_payload(message))
        if error:
            errors.append(f"slack: {error}")

    line_token = _env(LINE_CHANNEL_ACCESS_TOKEN_ENV)
    line_recipient = _env(LINE_RECIPIENT_ID_ENV)
    if line_token and line_recipient:
        error = _post_json(
            line_endpoint,
            line_payload(line_recipient, message),
            bearer_token=line_token,
        )
        if error:
            errors.append(f"line: {error}")

    return errors


def notify_skill_completed(
    skill: str,
    agent: str,
    run_id: str,
    output_preview: str | None = None,
) -> list[str]:
    """Convenience wrapper for Agentic OS skill runs."""
    return notify_success(
        title=f"Skill '{skill}' completed",
        detail=(
            f"Agent: {agent} · Run {run_id}"
            + (f"\n{output_preview.strip()[:300]}" if output_preview else "")
        ),
    )
