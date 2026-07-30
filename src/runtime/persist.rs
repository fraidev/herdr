//! Persist hub runtime membership (not remote AppState).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{RuntimeEntry, RuntimeId, RuntimeKind, RuntimeStatus};

const RUNTIMES_FILE: &str = "runtimes.json";

/// Membership fields only. Status and connection errors are live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRuntime {
    pub id: String,
    pub kind: RuntimeKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl From<&RuntimeEntry> for PersistedRuntime {
    fn from(entry: &RuntimeEntry) -> Self {
        Self {
            id: entry.id.as_str().to_string(),
            kind: entry.kind,
            label: entry.label.clone(),
            target: entry.target.clone(),
            session: entry.session.clone(),
        }
    }
}

impl PersistedRuntime {
    pub fn into_entry(self) -> Result<RuntimeEntry, String> {
        if self.kind == RuntimeKind::Local {
            return Err("persisted runtime cannot be local".into());
        }
        let id = RuntimeId::new(self.id).map_err(|()| "runtime id cannot be empty".to_string())?;
        Ok(RuntimeEntry {
            id,
            kind: self.kind,
            label: self.label,
            target: self.target,
            session: self.session,
            status: RuntimeStatus::Offline,
            last_error: None,
        })
    }
}

pub fn registry_path() -> PathBuf {
    crate::session::data_dir().join(RUNTIMES_FILE)
}

#[allow(dead_code)]
pub fn registry_path_for_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(RUNTIMES_FILE)
}

pub fn load_membership(path: &Path) -> Vec<PersistedRuntime> {
    match load_membership_strict(path) {
        Ok(entries) => entries,
        Err(err) => {
            warn!(
                path = %path.display(),
                err = %err,
                "failed to load runtime registry"
            );
            Vec::new()
        }
    }
}

pub fn load_membership_strict(path: &Path) -> std::io::Result<Vec<PersistedRuntime>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

pub fn save_membership(path: &Path, entries: &[PersistedRuntime]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(entries)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, format!("{json}\n"))?;
    #[cfg(windows)]
    if path.exists() {
        if let Err(err) = std::fs::remove_file(path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err);
        }
    }
    if let Err(err) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RuntimeKind, RuntimeRegistry};

    fn temp_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "herdr-runtimes-{name}-{}-{nanos}.json",
            std::process::id()
        ))
    }

    #[test]
    fn round_trip_membership() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);

        let mut registry = RuntimeRegistry::new();
        registry
            .add(RuntimeEntry {
                id: RuntimeId::new("workbox".to_string()).unwrap(),
                kind: RuntimeKind::RemoteSsh,
                label: "Workbox".into(),
                target: Some("workbox".into()),
                session: Some("worker".into()),
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .unwrap();
        registry
            .add(RuntimeEntry {
                id: RuntimeId::new("peer".to_string()).unwrap(),
                kind: RuntimeKind::Socket,
                label: "Peer".into(),
                target: Some("/tmp/peer.sock".into()),
                session: None,
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .unwrap();

        let membership = registry.membership_for_persist();
        save_membership(&path, &membership).unwrap();

        let loaded = load_membership_strict(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "peer");
        assert_eq!(loaded[0].kind, RuntimeKind::Socket);
        assert_eq!(loaded[1].id, "workbox");
        assert_eq!(loaded[1].session.as_deref(), Some("worker"));

        let restored = RuntimeRegistry::from_membership(loaded).unwrap();
        assert_eq!(restored.list().len(), 3);
        let workbox = restored.get("workbox").unwrap();
        assert_eq!(workbox.status, RuntimeStatus::Offline);
        assert_eq!(workbox.kind, RuntimeKind::RemoteSsh);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_is_empty() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(load_membership_strict(&path).unwrap().is_empty());
    }
}
