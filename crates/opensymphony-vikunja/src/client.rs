use std::time::Duration;

use crate::opensymphony_domain::{TrackerIssue, TrackerIssueStateSnapshot, TrackerIssueSummary};
use reqwest::{
    Client, Method, StatusCode,
    header::{ACCEPT, AUTHORIZATION, RETRY_AFTER},
};
use serde::de::DeserializeOwned;
use tokio::time::sleep;
use tracing::debug;

use super::error::VikunjaError;
use super::html::html_to_text;
use super::normalize::{normalize_task, normalize_task_state, normalize_task_summary, parse_datetime};
use super::rest::{VikunjaComment, VikunjaTask};

const DEFAULT_PAGE_SIZE: usize = 50;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INLINE_RATE_LIMIT_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VikunjaConfig {
    /// Vikunja instance base URL, e.g. `https://vikunja.example.com`.
    pub base_url: String,
    /// API token, sent as a bearer token.
    pub api_token: String,
    /// Numeric Vikunja project id whose tasks are polled.
    pub project_id: String,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    pub page_size: usize,
    pub request_timeout: Duration,
    pub retry_policy: RetryPolicy,
}

impl VikunjaConfig {
    pub fn new(
        base_url: impl Into<String>,
        api_token: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_token: api_token.into(),
            project_id: project_id.into(),
            active_states: Vec::new(),
            terminal_states: Vec::new(),
            page_size: DEFAULT_PAGE_SIZE,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkpadComment {
    pub id: String,
    pub body: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
pub struct VikunjaClient {
    http: Client,
    config: VikunjaConfig,
    authorization: String,
}

impl VikunjaClient {
    pub fn new(mut config: VikunjaConfig) -> Result<Self, VikunjaError> {
        if config.page_size == 0 {
            config.page_size = DEFAULT_PAGE_SIZE;
        }
        if config.request_timeout.is_zero() {
            config.request_timeout = DEFAULT_REQUEST_TIMEOUT;
        }
        if config.retry_policy.max_attempts == 0 {
            config.retry_policy.max_attempts = 1;
        }
        if config.retry_policy.initial_backoff.is_zero() {
            config.retry_policy.initial_backoff = Duration::from_millis(1);
        }
        if config.retry_policy.max_backoff < config.retry_policy.initial_backoff {
            config.retry_policy.max_backoff = config.retry_policy.initial_backoff;
        }
        config.base_url = normalize_required_string("tracker.endpoint", &config.base_url)?
            .trim_end_matches('/')
            .to_string();
        config.api_token = normalize_required_string("VIKUNJA_API_TOKEN", &config.api_token)?;
        config.project_id =
            normalize_required_string("tracker.project_slug", &config.project_id)?;
        if !config.project_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(VikunjaError::InvalidConfiguration(format!(
                "tracker.project_slug must be a numeric Vikunja project id, got `{}`",
                config.project_id
            )));
        }
        config.active_states =
            normalize_required_state_names("tracker.active_states", &config.active_states)?;
        config.terminal_states =
            normalize_required_state_names("tracker.terminal_states", &config.terminal_states)?;
        let authorization = format!("Bearer {}", config.api_token);

        let http = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| VikunjaError::InvalidConfiguration(error.to_string()))?;

        Ok(Self {
            http,
            config,
            authorization,
        })
    }

    pub async fn candidate_issues(&self) -> Result<Vec<TrackerIssue>, VikunjaError> {
        self.issues_by_state_names(&self.config.active_states).await
    }

    pub async fn candidate_issue_summaries(&self) -> Result<Vec<TrackerIssueSummary>, VikunjaError> {
        let active_states = self.config.active_states.clone();
        let tasks = self.tasks_by_state_names(&active_states).await?;
        tasks
            .into_iter()
            .map(|task| normalize_task_summary(task, &self.config.base_url))
            .collect()
    }

    pub async fn terminal_issues(&self) -> Result<Vec<TrackerIssue>, VikunjaError> {
        self.issues_by_state_names(&self.config.terminal_states)
            .await
    }

    pub async fn issues_by_state_names<S>(
        &self,
        state_names: &[S],
    ) -> Result<Vec<TrackerIssue>, VikunjaError>
    where
        S: AsRef<str>,
    {
        let tasks = self.tasks_by_state_names(state_names).await?;
        tasks
            .into_iter()
            .map(|task| normalize_task(task, &self.config.base_url))
            .collect()
    }

    async fn tasks_by_state_names<S>(
        &self,
        state_names: &[S],
    ) -> Result<Vec<VikunjaTask>, VikunjaError>
    where
        S: AsRef<str>,
    {
        let state_names = normalize_strings(state_names);
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        let wants_todo = state_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(super::normalize::STATE_TODO));
        let wants_done = state_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(super::normalize::STATE_DONE));
        if !wants_todo && !wants_done {
            return Ok(Vec::new());
        }

        let tasks = self.project_tasks().await?;
        Ok(tasks
            .into_iter()
            .filter(|task| if task.done { wants_done } else { wants_todo })
            .collect())
    }

    pub async fn issues_by_identifiers<S>(
        &self,
        identifiers: &[S],
    ) -> Result<Vec<TrackerIssue>, VikunjaError>
    where
        S: AsRef<str>,
    {
        self.project_issues_by_identifiers(identifiers).await
    }

    /// Resolve issues by their normalized identifiers. Identifiers outside the
    /// configured project are reported as missing, mirroring the Linear and
    /// Jira clients.
    pub async fn project_issues_by_identifiers<S>(
        &self,
        identifiers: &[S],
    ) -> Result<Vec<TrackerIssue>, VikunjaError>
    where
        S: AsRef<str>,
    {
        let identifiers = normalize_strings(identifiers);
        if identifiers.is_empty() {
            return Ok(Vec::new());
        }

        let tasks = self.project_tasks().await?;
        let mut issues_by_identifier = std::collections::HashMap::new();
        for task in tasks {
            let issue = normalize_task(task, &self.config.base_url)?;
            issues_by_identifier.insert(issue.identifier.to_ascii_uppercase(), issue);
        }

        let mut issues = Vec::new();
        let mut missing_issue_ids = Vec::new();
        for identifier in &identifiers {
            match issues_by_identifier.remove(&identifier.to_ascii_uppercase()) {
                Some(issue) => issues.push(issue),
                None => missing_issue_ids.push(identifier.clone()),
            }
        }

        if missing_issue_ids.is_empty() {
            Ok(issues)
        } else {
            Err(VikunjaError::MissingIssueIds {
                issue_ids: missing_issue_ids,
            })
        }
    }

    pub async fn issue_states_by_ids<S>(
        &self,
        issue_ids: &[S],
    ) -> Result<Vec<TrackerIssueStateSnapshot>, VikunjaError>
    where
        S: AsRef<str>,
    {
        let issue_ids = normalize_strings(issue_ids);
        if issue_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut snapshots = Vec::new();
        let mut missing_issue_ids = Vec::new();
        for issue_id in &issue_ids {
            let issue_id = validate_task_id(issue_id)?;
            match self
                .execute::<VikunjaTask>(
                    Method::GET,
                    &format!("/api/v1/tasks/{issue_id}"),
                    "task state lookup",
                )
                .await
            {
                Ok(task) => snapshots.push(normalize_task_state(task)?),
                Err(VikunjaError::HttpStatus {
                    status: StatusCode::NOT_FOUND,
                    ..
                }) => missing_issue_ids.push(issue_id.to_string()),
                Err(error) => return Err(error),
            }
        }

        if missing_issue_ids.is_empty() {
            Ok(snapshots)
        } else {
            Err(VikunjaError::MissingIssueIds {
                issue_ids: missing_issue_ids,
            })
        }
    }

    pub async fn fetch_workpad_comment(
        &self,
        issue_id: &str,
    ) -> Result<Option<WorkpadComment>, VikunjaError> {
        let issue_id = validate_task_id(issue_id)?;
        let comments: Vec<VikunjaComment> = self
            .execute(
                Method::GET,
                &format!("/api/v1/tasks/{issue_id}/comments"),
                "task comments",
            )
            .await?;

        let mut latest: Option<WorkpadComment> = None;
        for comment in comments {
            let Some(body) = html_to_text(&comment.comment) else {
                continue;
            };
            if !contains_workpad_marker(&body) {
                continue;
            }
            let updated_at = parse_datetime("updated", comment.updated.as_deref())?;
            let candidate = WorkpadComment {
                id: comment.id.to_string(),
                body,
                updated_at,
            };
            if latest
                .as_ref()
                .is_none_or(|existing| candidate.updated_at > existing.updated_at)
            {
                latest = Some(candidate);
            }
        }
        Ok(latest)
    }

    async fn project_tasks(&self) -> Result<Vec<VikunjaTask>, VikunjaError> {
        let mut tasks = Vec::new();
        let mut page = 1usize;

        loop {
            let path = format!(
                "/api/v1/projects/{}/tasks?page={page}&per_page={}",
                self.config.project_id, self.config.page_size
            );
            let batch: Vec<VikunjaTask> = self
                .execute(Method::GET, &path, "project tasks")
                .await?;
            let fetched = batch.len();
            tasks.extend(batch);
            if fetched < self.config.page_size {
                return Ok(tasks);
            }
            page += 1;
        }
    }

    async fn execute<T>(
        &self,
        method: Method,
        path_and_query: &str,
        operation: &str,
    ) -> Result<T, VikunjaError>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}{path_and_query}", self.config.base_url);
        let mut attempt = 1;

        loop {
            let request = self
                .http
                .request(method.clone(), &url)
                .header(AUTHORIZATION, &self.authorization)
                .header(ACCEPT, "application/json");

            let error = match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    let retry_after = parse_retry_delay(response.headers());
                    match response.text().await {
                        Ok(payload) if status.is_success() => {
                            return serde_json::from_str(&payload).map_err(|error| {
                                VikunjaError::InvalidResponse(format!(
                                    "failed to decode Vikunja response for {operation} after HTTP {status}: {error} (body_bytes={})",
                                    payload.len()
                                ))
                            });
                        }
                        Ok(payload) => VikunjaError::HttpStatus {
                            status,
                            body: payload,
                            retry_after,
                        },
                        Err(error) => VikunjaError::ResponseBody {
                            operation: operation.to_string(),
                            status,
                            retry_after,
                            source: Box::new(error),
                        },
                    }
                }
                Err(error) => VikunjaError::Request(Box::new(error)),
            };

            if self.should_retry(&error, attempt) {
                self.sleep_before_retry(&error, attempt).await;
                attempt += 1;
                continue;
            }
            return Err(error);
        }
    }

    fn should_retry(&self, error: &VikunjaError, attempt: usize) -> bool {
        if attempt >= self.config.retry_policy.max_attempts {
            return false;
        }

        let max_inline_rate_limit_retry = std::cmp::min(
            self.config.retry_policy.max_backoff,
            MAX_INLINE_RATE_LIMIT_RETRY,
        );
        if error.is_rate_limited()
            && error
                .retry_after()
                .is_some_and(|delay| delay > max_inline_rate_limit_retry)
        {
            return false;
        }

        match error {
            VikunjaError::Request(_) => true,
            VikunjaError::ResponseBody { .. } => true,
            VikunjaError::HttpStatus { status, .. } => {
                *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            VikunjaError::MissingIssueIds { .. }
            | VikunjaError::InvalidConfiguration(_)
            | VikunjaError::InvalidResponse(_) => false,
        }
    }

    async fn sleep_before_retry(&self, error: &VikunjaError, attempt: usize) {
        let mut delay = error
            .retry_after()
            .unwrap_or_else(|| self.exponential_backoff(attempt));
        if !error.is_rate_limited() {
            delay = delay.min(std::cmp::min(
                self.config.retry_policy.max_backoff,
                MAX_INLINE_RATE_LIMIT_RETRY,
            ));
        }
        debug!(
            attempt,
            delay_ms = delay.as_millis(),
            category = ?error.category(),
            "retrying Vikunja request"
        );
        sleep(delay).await;
    }

    fn exponential_backoff(&self, attempt: usize) -> Duration {
        let mut delay = self.config.retry_policy.initial_backoff;
        for _ in 1..attempt {
            match delay.checked_mul(2) {
                Some(next) if next <= self.config.retry_policy.max_backoff => delay = next,
                _ => return self.config.retry_policy.max_backoff,
            }
        }
        delay
    }
}

fn contains_workpad_marker(body: &str) -> bool {
    body.lines()
        .any(|line| line.trim_start().starts_with("## Agent Harness Workpad"))
}

fn normalize_required_string(field_name: &str, value: &str) -> Result<String, VikunjaError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        Err(VikunjaError::InvalidConfiguration(format!(
            "{field_name} must be a non-empty string"
        )))
    } else {
        Ok(normalized.to_string())
    }
}

fn normalize_required_state_names(
    field_name: &str,
    values: &[String],
) -> Result<Vec<String>, VikunjaError> {
    let normalized = normalize_strings(values);
    if normalized.is_empty() {
        Err(VikunjaError::InvalidConfiguration(format!(
            "{field_name} must contain at least one state name"
        )))
    } else {
        Ok(normalized)
    }
}

fn normalize_strings<S>(values: &[S]) -> Vec<String>
where
    S: AsRef<str>,
{
    let mut normalized = values
        .iter()
        .map(|value| value.as_ref().trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    normalized.dedup();
    normalized
}

// Task ids are interpolated into URL paths, so accept only the digits Vikunja
// itself uses rather than attempting to escape.
fn validate_task_id(value: &str) -> Result<&str, VikunjaError> {
    let trimmed = value.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|character| character.is_ascii_digit()) {
        Ok(trimmed)
    } else {
        Err(VikunjaError::InvalidConfiguration(format!(
            "Vikunja task id `{value}` must be numeric"
        )))
    }
}

fn parse_retry_delay(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    use super::{
        VikunjaClient, VikunjaConfig, contains_workpad_marker, parse_retry_delay, validate_task_id,
    };

    fn config() -> VikunjaConfig {
        let mut config = VikunjaConfig::new("https://vikunja.example.com/", "token", "7");
        config.active_states = vec!["Todo".to_string()];
        config.terminal_states = vec!["Done".to_string()];
        config
    }

    #[test]
    fn client_requires_a_numeric_project_id() {
        let mut invalid = config();
        invalid.project_id = "my-project".to_string();
        assert!(VikunjaClient::new(invalid).is_err());
        assert!(VikunjaClient::new(config()).is_ok());
    }

    #[test]
    fn task_ids_reject_path_injection() {
        assert_eq!(validate_task_id(" 42 ").ok(), Some("42"));
        assert!(validate_task_id("42/comments").is_err());
        assert!(validate_task_id("").is_err());
    }

    #[test]
    fn workpad_marker_detection_matches_other_trackers() {
        assert!(contains_workpad_marker(
            "intro\n  ## Agent Harness Workpad\nbody"
        ));
        assert!(!contains_workpad_marker("## Some other heading"));
    }

    #[test]
    fn retry_after_header_parses_as_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(parse_retry_delay(&headers), Some(Duration::from_secs(7)));
    }
}
