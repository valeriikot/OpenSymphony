# Success Notifications (Slack and LINE)

OpenSymphony can announce successfully implemented tickets to Slack and LINE.
When a worker run finishes with a succeeded outcome — on any harness
(OpenHands, Codex, or Claude Code) — the orchestrator posts a message like:

```
✅ COE-42 implemented successfully
Add retry backoff to the poller
Claude Code session completed (success) after 7 turn(s)
Harness: claude_code · Attempt 2
https://linear.app/acme/issue/COE-42
PR: https://github.com/acme/repo/pull/7
```

Delivery is strictly best-effort: notification failures are logged as
warnings and never affect the run outcome, retries, or issue state. Routing
dry-runs (`--dry-run`) do not notify.

## Slack

Create an [incoming webhook](https://api.slack.com/messaging/webhooks) for the
channel that should receive the messages, then export it in the environment
that runs `opensymphony run`:

```bash
export SLACK_WEBHOOK_URL="https://hooks.slack.com/services/T000/B000/XXXX"
```

## LINE

LINE messages go through the [Messaging API push
endpoint](https://developers.line.biz/en/reference/messaging-api/#send-push-message)
(LINE Notify was discontinued). You need a Messaging API channel and the ID of
the user, group, or room that should receive the push:

```bash
export LINE_CHANNEL_ACCESS_TOKEN="<channel access token>"
export LINE_RECIPIENT_ID="<user, group, or room id>"
```

Both variables are required; if only the token is set, LINE notifications stay
disabled and a warning is logged at startup. Message text is truncated to
LINE's 5000-character limit.

## Behavior notes

- Notifications fire per successful **run**. With conversation-reuse policies
  that run an issue through multiple continuation turns, each successful turn
  completion notifies; one-shot harness runs (Codex, Claude Code) notify once
  per issue attempt.
- Either channel can be configured alone; configuring neither disables the
  feature entirely (no outbound requests are made).
- Requests time out after 10 seconds; a failing channel does not block the
  other.
