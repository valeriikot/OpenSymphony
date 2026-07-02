//! Claude Code CLI harness adapter.
//!
//! Drives Anthropic's Claude Code CLI in headless mode
//! (`claude --print --output-format stream-json`). Each issue run is a
//! single non-interactive session: the rendered workflow prompt is written to
//! the child's stdin, newline-delimited JSON events stream back on stdout, and
//! the session ends with a terminal `result` event.

use serde_json::Value;

pub const CLAUDE_CODE_KIND: &str = "claude_code";
pub const CLAUDE_CODE_CONTRACT: &str = "claude-code-stream-json-v1";

const SUMMARY_PREVIEW_CHARS: usize = 160;

/// Command line for one headless Claude Code session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeLaunch {
    program: String,
    model: Option<String>,
}

impl ClaudeCodeLaunch {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            model: None,
        }
    }

    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    pub fn command_args(&self) -> Vec<String> {
        let mut args = vec![
            "--print".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            // Headless orchestrated runs cannot answer interactive
            // permission prompts, mirroring the Codex harness's
            // bypass-approvals posture inside isolated workspaces.
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ];
        if let Some(model) = &self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        args
    }

    pub fn to_command(&self) -> (String, Vec<String>) {
        (self.program.clone(), self.command_args())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizedClaudeEventKind {
    /// `type: "system", subtype: "init"` — session established.
    SystemInit,
    /// Other `type: "system"` events.
    System,
    /// `type: "assistant"` — assistant message (text and tool_use blocks).
    Assistant,
    /// `type: "user"` — tool results echoed back into the transcript.
    User,
    /// `type: "result"` — terminal event for the session.
    Result,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedClaudeEvent {
    pub kind: NormalizedClaudeEventKind,
    /// Raw `type` value, with the subtype appended for system events
    /// (e.g. `system.init`, `result`).
    pub event_type: String,
    pub session_id: Option<String>,
    pub event_id: Option<String>,
    pub token_usage: Option<ClaudeTokenUsage>,
    pub raw: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
}

impl NormalizedClaudeEvent {
    pub fn is_terminal(&self) -> bool {
        self.kind == NormalizedClaudeEventKind::Result
    }

    pub fn result_subtype(&self) -> Option<&str> {
        self.raw.get("subtype").and_then(Value::as_str)
    }

    pub fn result_is_error(&self) -> bool {
        self.raw
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// Normalizes one stream-json line from a headless Claude Code session.
/// Returns `None` for values that are not stream events (no string `type`).
pub fn normalize_stream_event(value: Value) -> Option<NormalizedClaudeEvent> {
    let event_type = value.get("type").and_then(Value::as_str)?.to_string();
    let subtype = value
        .get("subtype")
        .and_then(Value::as_str)
        .map(str::to_string);
    let kind = match (event_type.as_str(), subtype.as_deref()) {
        ("system", Some("init")) => NormalizedClaudeEventKind::SystemInit,
        ("system", _) => NormalizedClaudeEventKind::System,
        ("assistant", _) => NormalizedClaudeEventKind::Assistant,
        ("user", _) => NormalizedClaudeEventKind::User,
        ("result", _) => NormalizedClaudeEventKind::Result,
        _ => NormalizedClaudeEventKind::Other,
    };
    let qualified_type = match (&event_type, subtype) {
        (event_type, Some(subtype)) if event_type == "system" => {
            format!("{event_type}.{subtype}")
        }
        (event_type, _) => event_type.clone(),
    };
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|session_id| !session_id.trim().is_empty());
    let event_id = value
        .get("message")
        .and_then(|message| message.get("id"))
        .or_else(|| value.get("uuid"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let token_usage = claude_token_usage(&value);

    Some(NormalizedClaudeEvent {
        kind,
        event_type: qualified_type,
        session_id,
        event_id,
        token_usage,
        raw: value,
    })
}

/// Extracts token usage from a result event's top-level `usage` or an
/// assistant event's `message.usage`.
pub fn claude_token_usage(value: &Value) -> Option<ClaudeTokenUsage> {
    let usage = value.get("usage").or_else(|| {
        value
            .get("message")
            .and_then(|message| message.get("usage"))
    })?;
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input_tokens = read("input_tokens");
    let output_tokens = read("output_tokens");
    let cache_read_tokens = read("cache_read_input_tokens");
    let cache_creation_tokens = read("cache_creation_input_tokens");
    if input_tokens == 0 && output_tokens == 0 && cache_read_tokens == 0 {
        return None;
    }
    Some(ClaudeTokenUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        total_tokens: input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens,
    })
}

pub fn claude_event_summary(event: &NormalizedClaudeEvent) -> String {
    match event.kind {
        NormalizedClaudeEventKind::SystemInit => {
            let model = event
                .raw
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("<unknown model>");
            format!("Claude Code session started (model {model})")
        }
        NormalizedClaudeEventKind::Assistant => assistant_summary(&event.raw),
        NormalizedClaudeEventKind::User => "Tool results returned to Claude Code".to_string(),
        NormalizedClaudeEventKind::Result => {
            let subtype = event.result_subtype().unwrap_or("unknown");
            let turns = event
                .raw
                .get("num_turns")
                .and_then(Value::as_u64)
                .map(|turns| format!(" after {turns} turn(s)"))
                .unwrap_or_default();
            if event.result_is_error() {
                format!("Claude Code session failed ({subtype}){turns}")
            } else {
                format!("Claude Code session completed ({subtype}){turns}")
            }
        }
        NormalizedClaudeEventKind::System | NormalizedClaudeEventKind::Other => {
            format!("Claude Code event {}", event.event_type)
        }
    }
}

fn assistant_summary(raw: &Value) -> String {
    let Some(content) = raw
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return "Claude Code assistant message".to_string();
    };

    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        return format!("Claude: {}", bounded_preview(text));
                    }
                }
            }
            Some("tool_use") => {
                let tool = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown tool>");
                return format!("Claude invoked tool {tool}");
            }
            _ => {}
        }
    }
    "Claude Code assistant message".to_string()
}

fn bounded_preview(text: &str) -> String {
    let mut preview: String = text.chars().take(SUMMARY_PREVIEW_CHARS).collect();
    if text.chars().count() > SUMMARY_PREVIEW_CHARS {
        preview.push('…');
    }
    preview.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ClaudeCodeLaunch, NormalizedClaudeEventKind, claude_event_summary, normalize_stream_event,
    };

    #[test]
    fn launch_command_pins_headless_stream_json_flags() {
        let launch = ClaudeCodeLaunch::new("claude").with_model(Some("claude-sonnet-5".into()));
        let (program, args) = launch.to_command();

        assert_eq!(program, "claude");
        assert_eq!(
            args,
            vec![
                "--print",
                "--verbose",
                "--output-format",
                "stream-json",
                "--permission-mode",
                "bypassPermissions",
                "--model",
                "claude-sonnet-5",
            ]
        );
    }

    #[test]
    fn launch_command_omits_model_flag_when_unset() {
        let (_, args) = ClaudeCodeLaunch::new("claude").to_command();
        assert!(!args.contains(&"--model".to_string()));
    }

    #[test]
    fn init_events_normalize_with_session_id() {
        let event = normalize_stream_event(json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sess-1",
            "model": "claude-sonnet-5",
            "tools": ["Bash", "Edit"],
        }))
        .expect("init event should normalize");

        assert_eq!(event.kind, NormalizedClaudeEventKind::SystemInit);
        assert_eq!(event.event_type, "system.init");
        assert_eq!(event.session_id.as_deref(), Some("sess-1"));
        assert_eq!(
            claude_event_summary(&event),
            "Claude Code session started (model claude-sonnet-5)"
        );
    }

    #[test]
    fn assistant_events_summarize_text_and_tools() {
        let text_event = normalize_stream_event(json!({
            "type": "assistant",
            "session_id": "sess-1",
            "message": {
                "id": "msg-1",
                "content": [{"type": "text", "text": "  Working on the fix now.  "}],
                "usage": {"input_tokens": 100, "output_tokens": 25, "cache_read_input_tokens": 40}
            }
        }))
        .expect("assistant event should normalize");

        assert_eq!(text_event.kind, NormalizedClaudeEventKind::Assistant);
        assert_eq!(text_event.event_id.as_deref(), Some("msg-1"));
        assert_eq!(
            claude_event_summary(&text_event),
            "Claude: Working on the fix now."
        );
        let usage = text_event.token_usage.expect("usage should map");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 25);
        assert_eq!(usage.cache_read_tokens, 40);
        assert_eq!(usage.total_tokens, 165);

        let tool_event = normalize_stream_event(json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {}}]}
        }))
        .expect("tool event should normalize");
        assert_eq!(
            claude_event_summary(&tool_event),
            "Claude invoked tool Bash"
        );
    }

    #[test]
    fn result_events_are_terminal_and_expose_error_state() {
        let success = normalize_stream_event(json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "num_turns": 7,
            "session_id": "sess-1",
            "usage": {"input_tokens": 900, "output_tokens": 210, "cache_read_input_tokens": 0}
        }))
        .expect("result event should normalize");

        assert!(success.is_terminal());
        assert!(!success.result_is_error());
        assert_eq!(success.result_subtype(), Some("success"));
        assert_eq!(
            claude_event_summary(&success),
            "Claude Code session completed (success) after 7 turn(s)"
        );

        let failure = normalize_stream_event(json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
        }))
        .expect("result event should normalize");
        assert!(failure.result_is_error());
        assert_eq!(
            claude_event_summary(&failure),
            "Claude Code session failed (error_during_execution)"
        );
    }

    #[test]
    fn non_event_payloads_are_ignored() {
        assert!(normalize_stream_event(json!({"jsonrpc": "2.0", "id": 1})).is_none());
        assert!(normalize_stream_event(json!("just text")).is_none());
    }
}
