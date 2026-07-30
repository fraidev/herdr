use serde::{Deserialize, Serialize};

use crate::runtime::{RuntimeKind, RuntimeStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeTarget {
    pub runtime_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeAddParams {
    /// Stable runtime id (not `local`).
    pub id: String,
    pub kind: RuntimeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// SSH host for `remote_ssh`, or Unix socket path for `socket`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Optional remote Herdr session name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeInfo {
    pub runtime_id: String,
    pub kind: RuntimeKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub status: RuntimeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl From<&crate::runtime::RuntimeEntry> for RuntimeInfo {
    fn from(entry: &crate::runtime::RuntimeEntry) -> Self {
        Self {
            runtime_id: entry.id.as_str().to_string(),
            kind: entry.kind,
            label: entry.label.clone(),
            target: entry.target.clone(),
            session: entry.session.clone(),
            status: entry.status,
            last_error: entry.last_error.clone(),
        }
    }
}
