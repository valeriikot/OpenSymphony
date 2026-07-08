use chrono::{DateTime, Utc};

use crate::opensymphony_domain::{
    TrackerIssue, TrackerIssueBlocker, TrackerIssueRef, TrackerIssueState, TrackerIssueStateKind,
    TrackerIssueStateSnapshot, TrackerIssueSummary,
};

use super::error::VikunjaError;
use super::html::html_to_text;
use super::rest::{VikunjaRelatedTask, VikunjaTask};

/// Vikunja has no workflow statuses — a task is either open or done — so the
/// tracker-neutral state names are fixed. Workflow front matter must use them:
/// `active_states: [Todo]`, `terminal_states: [Done]`.
pub const STATE_TODO: &str = "Todo";
pub const STATE_DONE: &str = "Done";

const RELATION_BLOCKED: &str = "blocked";
const RELATION_SUBTASK: &str = "subtask";
const RELATION_PARENT: &str = "parenttask";

pub(super) fn normalize_task(task: VikunjaTask, base_url: &str) -> Result<TrackerIssue, VikunjaError> {
    let state = state_for(task.done);
    let identifier = normalize_identifier(&task.identifier, task.index);
    let related = task.related_tasks.unwrap_or_default();
    let blocked_by = related
        .get(RELATION_BLOCKED)
        .map(|blockers| normalize_blockers(blockers))
        .unwrap_or_default();
    let sub_issues = related
        .get(RELATION_SUBTASK)
        .map(|subtasks| normalize_sub_issues(subtasks, base_url))
        .unwrap_or_default();
    let parent = related
        .get(RELATION_PARENT)
        .and_then(|parents| parents.first())
        .map(|parent| issue_ref_from_related(parent, base_url));

    Ok(TrackerIssue {
        url: task_url(base_url, task.id),
        id: task.id.to_string(),
        identifier,
        title: task.title,
        description: html_to_text(&task.description),
        priority: normalize_priority(task.priority),
        state: state.name.clone(),
        state_kind: state.kind,
        // Vikunja does not attach branches or pull requests to tasks.
        branch_name: None,
        pr_url: None,
        labels: normalize_labels(task.labels.unwrap_or_default()),
        project_id: Some(task.project_id.to_string()),
        project_slug: None,
        project_name: None,
        parent_id: parent.as_ref().map(|parent| parent.id.clone()),
        parent,
        project_milestone: None,
        blocked_by,
        sub_issues,
        created_at: parse_datetime("created", task.created.as_deref())?,
        updated_at: parse_datetime("updated", task.updated.as_deref())?,
    })
}

pub(super) fn normalize_task_summary(
    task: VikunjaTask,
    base_url: &str,
) -> Result<TrackerIssueSummary, VikunjaError> {
    let issue = normalize_task(task, base_url)?;
    Ok(TrackerIssueSummary {
        id: issue.id,
        identifier: issue.identifier,
        url: issue.url,
        title: issue.title,
        priority: issue.priority,
        state: issue.state,
        state_kind: issue.state_kind,
        blocked_by: issue.blocked_by,
        sub_issues: issue.sub_issues,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
    })
}

pub(super) fn normalize_task_state(
    task: VikunjaTask,
) -> Result<TrackerIssueStateSnapshot, VikunjaError> {
    Ok(TrackerIssueStateSnapshot {
        state: state_for(task.done),
        updated_at: parse_datetime("updated", task.updated.as_deref())?,
        id: task.id.to_string(),
        identifier: normalize_identifier(&task.identifier, task.index),
    })
}

pub(super) fn state_for(done: bool) -> TrackerIssueState {
    if done {
        TrackerIssueState {
            id: "done".to_string(),
            name: STATE_DONE.to_string(),
            tracker_type: "completed".to_string(),
            kind: TrackerIssueStateKind::Completed,
        }
    } else {
        TrackerIssueState {
            id: "todo".to_string(),
            name: STATE_TODO.to_string(),
            tracker_type: "unstarted".to_string(),
            kind: TrackerIssueStateKind::Unstarted,
        }
    }
}

pub(super) fn state_name_for(done: bool) -> &'static str {
    if done { STATE_DONE } else { STATE_TODO }
}

/// Vikunja renders identifiers as `#12` for projects without an identifier
/// prefix and `PREFIX-12` otherwise. `#`-style identifiers are rewritten to
/// `TASK-12` so downstream consumers (workspace keys, branch labels) get a
/// conventional `KEY-number` shape.
pub(super) fn normalize_identifier(raw: &str, index: i64) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return format!("TASK-{index}");
    }
    match trimmed.strip_prefix('#') {
        Some(rest) if !rest.trim().is_empty() => format!("TASK-{}", rest.trim()),
        Some(_) => format!("TASK-{index}"),
        None => trimmed.to_string(),
    }
}

pub(super) fn task_url(base_url: &str, task_id: i64) -> String {
    format!("{}/tasks/{task_id}", base_url.trim_end_matches('/'))
}

// Vikunja priorities: 0 unset, 1 low, 2 medium, 3 high, 4 urgent, 5 DO NOW.
// Domain scale: 1 urgent .. 4 low.
fn normalize_priority(priority: Option<i64>) -> Option<u8> {
    match priority? {
        5 | 4 => Some(1),
        3 => Some(2),
        2 => Some(3),
        1 => Some(4),
        _ => None,
    }
}

fn normalize_labels(labels: Vec<super::rest::VikunjaLabel>) -> Vec<String> {
    let mut labels = labels
        .into_iter()
        .map(|label| label.title.trim().to_string())
        .filter(|title| !title.is_empty())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    labels
}

fn normalize_blockers(blockers: &[VikunjaRelatedTask]) -> Vec<TrackerIssueBlocker> {
    let mut blockers = blockers
        .iter()
        .map(|blocker| TrackerIssueBlocker {
            id: blocker.id.to_string(),
            identifier: normalize_identifier(&blocker.identifier, blocker.index),
            title: blocker.title.clone(),
            state: state_for(blocker.done),
        })
        .collect::<Vec<_>>();
    blockers.sort_by(|left, right| left.identifier.cmp(&right.identifier));
    blockers.dedup_by(|left, right| left.id == right.id);
    blockers
}

fn normalize_sub_issues(subtasks: &[VikunjaRelatedTask], base_url: &str) -> Vec<TrackerIssueRef> {
    let mut sub_issues = subtasks
        .iter()
        .map(|subtask| issue_ref_from_related(subtask, base_url))
        .collect::<Vec<_>>();
    sub_issues.sort_by(|left, right| left.identifier.cmp(&right.identifier));
    sub_issues.dedup_by(|left, right| left.id == right.id);
    sub_issues
}

fn issue_ref_from_related(related: &VikunjaRelatedTask, base_url: &str) -> TrackerIssueRef {
    TrackerIssueRef {
        title: (!related.title.trim().is_empty()).then(|| related.title.clone()),
        id: related.id.to_string(),
        identifier: normalize_identifier(&related.identifier, related.index),
        url: Some(task_url(base_url, related.id)),
        state: state_name_for(related.done).to_string(),
    }
}

pub(super) fn parse_datetime(
    field: &str,
    value: Option<&str>,
) -> Result<DateTime<Utc>, VikunjaError> {
    let value = value.ok_or_else(|| {
        VikunjaError::InvalidResponse(format!("Vikunja task omitted the `{field}` timestamp"))
    })?;
    DateTime::parse_from_rfc3339(value.trim())
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| {
            VikunjaError::InvalidResponse(format!(
                "Vikunja task returned an unparseable `{field}` timestamp: {value}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::opensymphony_domain::TrackerIssueStateKind;

    use super::super::rest::{VikunjaLabel, VikunjaRelatedTask, VikunjaTask};
    use super::{normalize_identifier, normalize_task, parse_datetime};

    fn task(id: i64, identifier: &str, done: bool) -> VikunjaTask {
        VikunjaTask {
            id,
            title: format!("Task {id}"),
            description: String::new(),
            done,
            priority: Some(4),
            labels: Some(vec![
                VikunjaLabel {
                    title: "bug".to_string(),
                },
                VikunjaLabel {
                    title: "bug".to_string(),
                },
            ]),
            identifier: identifier.to_string(),
            index: id,
            project_id: 7,
            created: Some("2026-05-01T10:00:00Z".to_string()),
            updated: Some("2026-05-02T10:00:00Z".to_string()),
            related_tasks: None,
        }
    }

    #[test]
    fn hash_identifiers_normalize_to_task_prefix() {
        assert_eq!(normalize_identifier("#12", 12), "TASK-12");
        assert_eq!(normalize_identifier("VIK-12", 12), "VIK-12");
        assert_eq!(normalize_identifier("", 3), "TASK-3");
        assert_eq!(normalize_identifier("#", 3), "TASK-3");
    }

    #[test]
    fn done_flag_maps_to_fixed_states() {
        let open = normalize_task(task(1, "#1", false), "https://vikunja.example.com")
            .expect("task should normalize");
        let done = normalize_task(task(2, "#2", true), "https://vikunja.example.com")
            .expect("task should normalize");

        assert_eq!(open.state, "Todo");
        assert_eq!(open.state_kind, TrackerIssueStateKind::Unstarted);
        assert_eq!(done.state, "Done");
        assert_eq!(done.state_kind, TrackerIssueStateKind::Completed);
        assert_eq!(open.priority, Some(1));
        assert_eq!(open.labels, vec!["bug".to_string()]);
        assert_eq!(open.url, "https://vikunja.example.com/tasks/1");
    }

    #[test]
    fn blocked_and_subtask_relations_map_to_domain_fields() {
        let mut related = HashMap::new();
        related.insert(
            "blocked".to_string(),
            vec![VikunjaRelatedTask {
                id: 9,
                title: "Blocker".to_string(),
                done: true,
                identifier: "#9".to_string(),
                index: 9,
            }],
        );
        related.insert(
            "subtask".to_string(),
            vec![VikunjaRelatedTask {
                id: 10,
                title: "Child".to_string(),
                done: false,
                identifier: "#10".to_string(),
                index: 10,
            }],
        );
        let mut raw = task(1, "#1", false);
        raw.related_tasks = Some(related);

        let issue =
            normalize_task(raw, "https://vikunja.example.com").expect("task should normalize");

        assert_eq!(issue.blocked_by.len(), 1);
        assert_eq!(issue.blocked_by[0].identifier, "TASK-9");
        assert!(issue.blocked_by[0].state.kind.is_terminal());
        assert_eq!(issue.sub_issues.len(), 1);
        assert_eq!(issue.sub_issues[0].identifier, "TASK-10");
        assert_eq!(issue.sub_issues[0].state, "Todo");
    }

    #[test]
    fn timestamps_require_rfc3339() {
        assert!(parse_datetime("created", Some("2026-05-01T10:00:00Z")).is_ok());
        assert!(parse_datetime("created", Some("yesterday")).is_err());
        assert!(parse_datetime("created", None).is_err());
    }
}
