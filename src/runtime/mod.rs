//! Hub-side multi-runtime federation registry.
//!
//! The local `herdr server` is the hub. Remote hosts keep their own servers;
//! this module tracks membership and live status only. It does not merge PTYs
//! or remote AppState.

mod ids;
mod persist;

// ID helpers are exercised by unit tests today; inventory merge (PR3+) will use them
// from other crates modules via `crate::runtime::`.
#[allow(unused_imports)]
pub use ids::{
    is_scoped_remote_target, parse_runtime_id, scope_target, unscope_target, validate_runtime_id,
};
#[allow(unused_imports)]
pub use persist::{
    load_membership, load_membership_strict, registry_path, registry_path_for_data_dir,
    save_membership, PersistedRuntime,
};

use serde::{Deserialize, Serialize};

/// Stable runtime identifier. `local` is reserved for the hub itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeId(String);

impl RuntimeId {
    pub fn new(id: String) -> Result<Self, ()> {
        if id.is_empty() {
            return Err(());
        }
        Ok(Self(id))
    }

    pub fn local() -> Self {
        Self(LOCAL_RUNTIME_ID.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_local(&self) -> bool {
        self.0 == LOCAL_RUNTIME_ID
    }
}

impl std::fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RuntimeId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

pub const LOCAL_RUNTIME_ID: &str = "local";
pub const LOCAL_RUNTIME_LABEL: &str = "Local";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Local,
    Socket,
    RemoteSsh,
}

impl RuntimeKind {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Socket => "socket",
            Self::RemoteSsh => "remote_ssh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Connected,
    Degraded,
    Offline,
    Connecting,
}

impl RuntimeStatus {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Degraded => "degraded",
            Self::Offline => "offline",
            Self::Connecting => "connecting",
        }
    }
}

/// One runtime known to the hub (local always present).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEntry {
    pub id: RuntimeId,
    pub kind: RuntimeKind,
    pub label: String,
    /// SSH host alias/hostname for [`RuntimeKind::RemoteSsh`], or Unix socket
    /// path for [`RuntimeKind::Socket`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Optional remote Herdr session name (`--session`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub status: RuntimeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl RuntimeEntry {
    pub fn local() -> Self {
        Self {
            id: RuntimeId::local(),
            kind: RuntimeKind::Local,
            label: LOCAL_RUNTIME_LABEL.to_string(),
            target: None,
            session: None,
            status: RuntimeStatus::Connected,
            last_error: None,
        }
    }
}

/// Hub registry of local + remote runtimes. Local is always first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRegistry {
    entries: Vec<RuntimeEntry>,
}

impl Default for RuntimeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeRegistry {
    pub fn new() -> Self {
        Self {
            entries: vec![RuntimeEntry::local()],
        }
    }

    /// Build a registry from persisted membership (non-local only).
    pub fn from_membership(membership: Vec<PersistedRuntime>) -> Result<Self, String> {
        let mut registry = Self::new();
        for persisted in membership {
            let entry = persisted.into_entry()?;
            registry.add(entry)?;
        }
        Ok(registry)
    }

    pub fn list(&self) -> &[RuntimeEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&RuntimeEntry> {
        self.entries.iter().find(|entry| entry.id.as_str() == id)
    }

    #[allow(dead_code)] // connection health updates land in PR2+
    pub fn get_mut(&mut self, id: &str) -> Option<&mut RuntimeEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.id.as_str() == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// Add a non-local runtime. Fails if the id is reserved, invalid, or taken.
    pub fn add(&mut self, entry: RuntimeEntry) -> Result<&RuntimeEntry, String> {
        if entry.kind == RuntimeKind::Local || entry.id.is_local() {
            return Err("cannot add a second local runtime".into());
        }
        validate_runtime_id(entry.id.as_str())?;
        if self.contains(entry.id.as_str()) {
            return Err(format!("runtime '{}' already exists", entry.id));
        }
        if entry.kind == RuntimeKind::RemoteSsh {
            let target = entry.target.as_deref().unwrap_or("").trim();
            if target.is_empty() {
                return Err("remote_ssh runtime requires a non-empty target (SSH host)".into());
            }
        }
        if entry.kind == RuntimeKind::Socket {
            let target = entry.target.as_deref().unwrap_or("").trim();
            if target.is_empty() {
                return Err("socket runtime requires a non-empty target (socket path)".into());
            }
        }
        self.entries.push(entry);
        let index = self.entries.len() - 1;
        Ok(&self.entries[index])
    }

    /// Remove a non-local runtime by id.
    pub fn remove(&mut self, id: &str) -> Result<RuntimeEntry, String> {
        if id == LOCAL_RUNTIME_ID {
            return Err("cannot remove the local runtime".into());
        }
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id.as_str() == id)
            .ok_or_else(|| format!("runtime '{id}' not found"))?;
        Ok(self.entries.remove(index))
    }

    /// Membership rows suitable for disk (excludes local).
    /// Sorted by id for stable on-disk representation.
    pub fn membership_for_persist(&self) -> Vec<PersistedRuntime> {
        let mut membership: Vec<PersistedRuntime> = self
            .entries
            .iter()
            .filter(|entry| !entry.id.is_local())
            .map(PersistedRuntime::from)
            .collect();
        membership.sort_by(|left, right| left.id.cmp(&right.id));
        membership
    }

    #[allow(dead_code)] // connection health updates land in PR2+
    pub fn set_status(
        &mut self,
        id: &str,
        status: RuntimeStatus,
        last_error: Option<String>,
    ) -> Result<(), String> {
        let entry = self
            .get_mut(id)
            .ok_or_else(|| format!("runtime '{id}' not found"))?;
        if entry.id.is_local() && status != RuntimeStatus::Connected {
            return Err("local runtime status is always connected".into());
        }
        entry.status = status;
        entry.last_error = last_error;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_has_local_connected() {
        let registry = RuntimeRegistry::new();
        assert_eq!(registry.list().len(), 1);
        let local = registry.get(LOCAL_RUNTIME_ID).unwrap();
        assert!(local.id.is_local());
        assert_eq!(local.kind, RuntimeKind::Local);
        assert_eq!(local.status, RuntimeStatus::Connected);
        assert_eq!(local.label, LOCAL_RUNTIME_LABEL);
    }

    #[test]
    fn add_and_remove_remote_entries() {
        let mut registry = RuntimeRegistry::new();
        registry
            .add(RuntimeEntry {
                id: RuntimeId::new("workbox".to_string()).unwrap(),
                kind: RuntimeKind::RemoteSsh,
                label: "Workbox".into(),
                target: Some("workbox.example".into()),
                session: Some("worker".into()),
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .unwrap();
        registry
            .add(RuntimeEntry {
                id: RuntimeId::new("peer".to_string()).unwrap(),
                kind: RuntimeKind::Socket,
                label: "Peer session".into(),
                target: Some("/tmp/herdr-peer.sock".into()),
                session: None,
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .unwrap();

        assert_eq!(registry.list().len(), 3);
        assert!(registry.contains("workbox"));
        assert!(registry.contains("peer"));

        let removed = registry.remove("workbox").unwrap();
        assert_eq!(removed.id.as_str(), "workbox");
        assert!(!registry.contains("workbox"));
        assert!(registry.contains("peer"));
        assert!(registry.contains(LOCAL_RUNTIME_ID));
    }

    #[test]
    fn rejects_duplicate_and_local_mutations() {
        let mut registry = RuntimeRegistry::new();
        let entry = RuntimeEntry {
            id: RuntimeId::new("workbox".to_string()).unwrap(),
            kind: RuntimeKind::RemoteSsh,
            label: "Workbox".into(),
            target: Some("host".into()),
            session: None,
            status: RuntimeStatus::Offline,
            last_error: None,
        };
        registry.add(entry.clone()).unwrap();
        assert!(registry.add(entry).is_err());
        assert!(registry
            .add(RuntimeEntry {
                id: RuntimeId::local(),
                kind: RuntimeKind::Local,
                label: "x".into(),
                target: None,
                session: None,
                status: RuntimeStatus::Connected,
                last_error: None,
            })
            .is_err());
        assert!(registry.remove(LOCAL_RUNTIME_ID).is_err());
    }

    #[test]
    fn requires_target_for_socket_and_ssh() {
        let mut registry = RuntimeRegistry::new();
        assert!(registry
            .add(RuntimeEntry {
                id: RuntimeId::new("ssh1".to_string()).unwrap(),
                kind: RuntimeKind::RemoteSsh,
                label: "SSH".into(),
                target: None,
                session: None,
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .is_err());
        assert!(registry
            .add(RuntimeEntry {
                id: RuntimeId::new("sock1".to_string()).unwrap(),
                kind: RuntimeKind::Socket,
                label: "Sock".into(),
                target: Some("  ".into()),
                session: None,
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .is_err());
    }

    #[test]
    fn membership_excludes_local() {
        let mut registry = RuntimeRegistry::new();
        registry
            .add(RuntimeEntry {
                id: RuntimeId::new("workbox".to_string()).unwrap(),
                kind: RuntimeKind::RemoteSsh,
                label: "Workbox".into(),
                target: Some("host".into()),
                session: None,
                status: RuntimeStatus::Connected,
                last_error: Some("should not persist".into()),
            })
            .unwrap();
        let membership = registry.membership_for_persist();
        assert_eq!(membership.len(), 1);
        assert_eq!(membership[0].id, "workbox");
        // status/last_error are not part of PersistedRuntime
        let restored = RuntimeRegistry::from_membership(membership).unwrap();
        let workbox = restored.get("workbox").unwrap();
        assert_eq!(workbox.status, RuntimeStatus::Offline);
        assert!(workbox.last_error.is_none());
    }

    #[test]
    fn set_status_updates_non_local() {
        let mut registry = RuntimeRegistry::new();
        registry
            .add(RuntimeEntry {
                id: RuntimeId::new("workbox".to_string()).unwrap(),
                kind: RuntimeKind::RemoteSsh,
                label: "Workbox".into(),
                target: Some("host".into()),
                session: None,
                status: RuntimeStatus::Offline,
                last_error: None,
            })
            .unwrap();
        registry
            .set_status(
                "workbox",
                RuntimeStatus::Degraded,
                Some("ping failed".into()),
            )
            .unwrap();
        let entry = registry.get("workbox").unwrap();
        assert_eq!(entry.status, RuntimeStatus::Degraded);
        assert_eq!(entry.last_error.as_deref(), Some("ping failed"));
        assert!(registry
            .set_status(LOCAL_RUNTIME_ID, RuntimeStatus::Offline, None)
            .is_err());
    }
}
