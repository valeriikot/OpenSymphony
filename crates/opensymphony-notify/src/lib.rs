//! Outbound notifications for orchestrator milestones.
//!
//! Sends a message to Slack (incoming webhook) and/or LINE (Messaging API
//! push) when an issue run completes successfully. Notification delivery is
//! strictly best-effort: failures are reported to the caller for logging and
//! must never affect the run outcome.

use std::time::Duration;

use serde_json::{Value, json};

pub const SLACK_WEBHOOK_URL_ENV: &str = "SLACK_WEBHOOK_URL";
pub const LINE_CHANNEL_ACCESS_TOKEN_ENV: &str = "LINE_CHANNEL_ACCESS_TOKEN";
pub const LINE_RECIPIENT_ID_ENV: &str = "LINE_RECIPIENT_ID";

pub const LINE_PUSH_ENDPOINT: &str = "https://api.line.me/v2/bot/message/push";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// LINE rejects text messages longer than 5000 characters.
const LINE_TEXT_LIMIT_CHARS: usize = 5000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationConfig {
    pub slack_webhook_url: Option<String>,
    pub line: Option<LineConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineConfig {
    pub channel_access_token: String,
    pub recipient_id: String,
}

impl NotificationConfig {
    /// Builds the config from process environment variables. Slack activates
    /// with `SLACK_WEBHOOK_URL`; LINE requires both `LINE_CHANNEL_ACCESS_TOKEN`
    /// and `LINE_RECIPIENT_ID` (a user, group, or room id for the push API).
    pub fn from_process_env() -> Self {
        let read = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let line = match (
            read(LINE_CHANNEL_ACCESS_TOKEN_ENV),
            read(LINE_RECIPIENT_ID_ENV),
        ) {
            (Some(channel_access_token), Some(recipient_id)) => Some(LineConfig {
                channel_access_token,
                recipient_id,
            }),
            (Some(_), None) => {
                tracing::warn!(
                    "{LINE_CHANNEL_ACCESS_TOKEN_ENV} is set but {LINE_RECIPIENT_ID_ENV} is not; LINE notifications stay disabled"
                );
                None
            }
            _ => None,
        };
        Self {
            slack_webhook_url: read(SLACK_WEBHOOK_URL_ENV),
            line,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.slack_webhook_url.is_some() || self.line.is_some()
    }
}

/// Details of a successfully implemented ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueCompletionNotification {
    pub identifier: String,
    pub title: String,
    pub url: Option<String>,
    pub pr_url: Option<String>,
    pub harness_kind: String,
    pub attempt: Option<u32>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyChannel {
    Slack,
    Line,
}

impl std::fmt::Display for NotifyChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slack => write!(f, "slack"),
            Self::Line => write!(f, "line"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("{channel} notification request failed: {source}")]
    Request {
        channel: NotifyChannel,
        #[source]
        source: reqwest::Error,
    },
    #[error("{channel} notification rejected with HTTP {status}: {body}")]
    HttpStatus {
        channel: NotifyChannel,
        status: reqwest::StatusCode,
        body: String,
    },
}

/// Outcome of one notification fan-out; `None` per channel means the channel
/// is not configured.
#[derive(Debug, Default)]
pub struct NotificationDelivery {
    pub slack: Option<Result<(), NotifyError>>,
    pub line: Option<Result<(), NotifyError>>,
}

impl NotificationDelivery {
    pub fn errors(&self) -> impl Iterator<Item = &NotifyError> {
        self.slack
            .iter()
            .chain(self.line.iter())
            .filter_map(|result| result.as_ref().err())
    }
}

#[derive(Debug, Clone)]
pub struct Notifier {
    config: NotificationConfig,
    client: reqwest::Client,
    line_endpoint: String,
}

impl Notifier {
    pub fn new(config: NotificationConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            config,
            client,
            line_endpoint: LINE_PUSH_ENDPOINT.to_string(),
        }
    }

    pub fn from_process_env() -> Self {
        Self::new(NotificationConfig::from_process_env())
    }

    /// Overrides the LINE push endpoint (tests only).
    pub fn with_line_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.line_endpoint = endpoint.into();
        self
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_enabled()
    }

    /// Sends the ticket-implemented message to every configured channel.
    /// Always best-effort: one channel failing does not stop the other.
    pub async fn notify_issue_completed(
        &self,
        notification: &IssueCompletionNotification,
    ) -> NotificationDelivery {
        let message = completion_message(notification);
        let mut delivery = NotificationDelivery::default();
        if let Some(webhook_url) = &self.config.slack_webhook_url {
            delivery.slack = Some(
                self.post_json(
                    NotifyChannel::Slack,
                    webhook_url,
                    None,
                    &slack_payload(&message),
                )
                .await,
            );
        }
        if let Some(line) = &self.config.line {
            delivery.line = Some(
                self.post_json(
                    NotifyChannel::Line,
                    &self.line_endpoint,
                    Some(&line.channel_access_token),
                    &line_payload(&line.recipient_id, &message),
                )
                .await,
            );
        }
        delivery
    }

    async fn post_json(
        &self,
        channel: NotifyChannel,
        url: &str,
        bearer_token: Option<&str>,
        payload: &Value,
    ) -> Result<(), NotifyError> {
        let mut request = self.client.post(url).json(payload);
        if let Some(token) = bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|source| NotifyError::Request { channel, source })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(NotifyError::HttpStatus {
            channel,
            status,
            body: body.chars().take(300).collect(),
        })
    }
}

/// Human-readable message shared by all channels.
pub fn completion_message(notification: &IssueCompletionNotification) -> String {
    let mut message = format!(
        "✅ {} implemented successfully\n{}",
        notification.identifier,
        notification.title.trim()
    );
    if let Some(summary) = notification
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        message.push('\n');
        message.push_str(summary);
    }
    let mut context = format!("Harness: {}", notification.harness_kind);
    if let Some(attempt) = notification.attempt {
        context.push_str(&format!(" · Attempt {attempt}"));
    }
    message.push('\n');
    message.push_str(&context);
    if let Some(url) = notification
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        message.push('\n');
        message.push_str(url);
    }
    if let Some(pr_url) = notification
        .pr_url
        .as_deref()
        .map(str::trim)
        .filter(|pr_url| !pr_url.is_empty())
    {
        message.push('\n');
        message.push_str("PR: ");
        message.push_str(pr_url);
    }
    message
}

pub fn slack_payload(message: &str) -> Value {
    json!({ "text": message })
}

pub fn line_payload(recipient_id: &str, message: &str) -> Value {
    let text: String = message.chars().take(LINE_TEXT_LIMIT_CHARS).collect();
    json!({
        "to": recipient_id,
        "messages": [{ "type": "text", "text": text }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notification() -> IssueCompletionNotification {
        IssueCompletionNotification {
            identifier: "COE-42".into(),
            title: "Add retry backoff".into(),
            url: Some("https://linear.app/acme/issue/COE-42".into()),
            pr_url: Some("https://github.com/acme/repo/pull/7".into()),
            harness_kind: "claude_code".into(),
            attempt: Some(2),
            summary: Some("Claude Code session completed (success) after 7 turn(s)".into()),
        }
    }

    #[test]
    fn completion_message_includes_ticket_links_and_context() {
        let message = completion_message(&sample_notification());

        assert!(message.starts_with("✅ COE-42 implemented successfully"));
        assert!(message.contains("Add retry backoff"));
        assert!(message.contains("Claude Code session completed"));
        assert!(message.contains("Harness: claude_code · Attempt 2"));
        assert!(message.contains("https://linear.app/acme/issue/COE-42"));
        assert!(message.contains("PR: https://github.com/acme/repo/pull/7"));
    }

    #[test]
    fn completion_message_omits_absent_optional_fields() {
        let notification = IssueCompletionNotification {
            url: None,
            pr_url: None,
            summary: None,
            attempt: None,
            ..sample_notification()
        };
        let message = completion_message(&notification);

        assert!(message.contains("Harness: claude_code"));
        assert!(!message.contains("Attempt"));
        assert!(!message.contains("PR:"));
        assert!(!message.contains("https://"));
    }

    #[test]
    fn slack_payload_wraps_message_as_text() {
        let payload = slack_payload("hello");
        assert_eq!(payload, json!({ "text": "hello" }));
    }

    #[test]
    fn line_payload_targets_recipient_and_truncates_to_limit() {
        let long = "あ".repeat(LINE_TEXT_LIMIT_CHARS + 100);
        let payload = line_payload("U1234", &long);

        assert_eq!(payload["to"], "U1234");
        assert_eq!(payload["messages"][0]["type"], "text");
        let text = payload["messages"][0]["text"].as_str().expect("text");
        assert_eq!(text.chars().count(), LINE_TEXT_LIMIT_CHARS);
    }

    #[test]
    fn config_requires_both_line_variables() {
        let config = NotificationConfig {
            slack_webhook_url: None,
            line: None,
        };
        assert!(!config.is_enabled());

        let config = NotificationConfig {
            slack_webhook_url: Some("https://hooks.slack.com/services/T/B/x".into()),
            line: None,
        };
        assert!(config.is_enabled());
    }

    #[tokio::test]
    async fn notify_posts_to_slack_and_line_with_expected_shapes() {
        use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
        use std::sync::{Arc, Mutex};

        type CapturedRequest = (String, Option<String>, Value);

        #[derive(Clone, Default)]
        struct Captured {
            requests: Arc<Mutex<Vec<CapturedRequest>>>,
        }

        async fn capture(
            State(captured): State<Captured>,
            headers: HeaderMap,
            axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
            Json(body): Json<Value>,
        ) -> &'static str {
            let auth = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            captured.requests.lock().expect("capture lock").push((
                uri.path().to_string(),
                auth,
                body,
            ));
            "{}"
        }

        let captured = Captured::default();
        let app = Router::new()
            .route("/slack", post(capture))
            .route("/line", post(capture))
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let notifier = Notifier::new(NotificationConfig {
            slack_webhook_url: Some(format!("{base}/slack")),
            line: Some(LineConfig {
                channel_access_token: "line-token".into(),
                recipient_id: "U777".into(),
            }),
        })
        .with_line_endpoint(format!("{base}/line"));

        let delivery = notifier
            .notify_issue_completed(&sample_notification())
            .await;

        assert!(matches!(delivery.slack, Some(Ok(()))));
        assert!(matches!(delivery.line, Some(Ok(()))));
        let requests = captured.requests.lock().expect("capture lock").clone();
        assert_eq!(requests.len(), 2);
        let slack = requests
            .iter()
            .find(|(path, _, _)| path == "/slack")
            .expect("slack request");
        assert!(slack.1.is_none());
        assert!(
            slack.2["text"]
                .as_str()
                .expect("slack text")
                .contains("COE-42")
        );
        let line = requests
            .iter()
            .find(|(path, _, _)| path == "/line")
            .expect("line request");
        assert_eq!(line.1.as_deref(), Some("Bearer line-token"));
        assert_eq!(line.2["to"], "U777");
        assert!(
            line.2["messages"][0]["text"]
                .as_str()
                .expect("line text")
                .contains("COE-42")
        );
    }

    #[tokio::test]
    async fn notify_reports_http_failures_without_panicking() {
        use axum::{Router, routing::post};

        let app = Router::new().route(
            "/slack",
            post(|| async { (axum::http::StatusCode::FORBIDDEN, "invalid_token") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let notifier = Notifier::new(NotificationConfig {
            slack_webhook_url: Some(format!("{base}/slack")),
            line: None,
        });

        let delivery = notifier
            .notify_issue_completed(&sample_notification())
            .await;

        assert_eq!(delivery.errors().count(), 1);
        let error = delivery.errors().next().expect("error");
        assert!(error.to_string().contains("403"));
        assert!(error.to_string().contains("invalid_token"));
    }
}
