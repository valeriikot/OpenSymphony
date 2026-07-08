//! Serde DTOs for the Vikunja REST API v1 payloads consumed by the client.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct VikunjaTask {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub labels: Option<Vec<VikunjaLabel>>,
    /// Server-computed human identifier, e.g. `#12` or `PREFIX-12` when the
    /// project defines an identifier prefix.
    #[serde(default)]
    pub identifier: String,
    /// Per-project task index (the `12` in `#12`).
    #[serde(default)]
    pub index: i64,
    #[serde(default)]
    pub project_id: i64,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    /// Relation kind (`subtask`, `parenttask`, `blocked`, ...) to related
    /// tasks. Only present on single-task reads.
    #[serde(default)]
    pub related_tasks: Option<HashMap<String, Vec<VikunjaRelatedTask>>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VikunjaRelatedTask {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub index: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VikunjaLabel {
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VikunjaComment {
    pub id: i64,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub updated: Option<String>,
}
