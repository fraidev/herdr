use super::responses::{encode_error, encode_success};
use crate::api::schema::{ResponseResult, RuntimeAddParams, RuntimeInfo, RuntimeTarget};
use crate::app::App;
use crate::runtime::{
    connected_status, offline_status, parse_runtime_id, RuntimeEntry, RuntimeKind, RuntimeRegistry,
    RuntimeStatus,
};

impl App {
    pub(super) fn handle_runtime_list(&mut self, id: String) -> String {
        let runtimes = self
            .runtime_registry
            .list()
            .iter()
            .map(RuntimeInfo::from)
            .collect();
        encode_success(id, ResponseResult::RuntimeList { runtimes })
    }

    pub(super) fn handle_runtime_connect(&mut self, id: String, target: RuntimeTarget) -> String {
        if target.runtime_id == crate::runtime::LOCAL_RUNTIME_ID {
            return encode_error(id, "invalid_runtime", "local runtime is always connected");
        }
        let Some(entry) = self.runtime_registry.get(&target.runtime_id).cloned() else {
            return runtime_not_found(id, &target.runtime_id);
        };

        let _ =
            self.runtime_registry
                .set_status(&target.runtime_id, RuntimeStatus::Connecting, None);

        match self.runtime_connections.connect(&entry) {
            Ok(_) => {
                let (status, err) = connected_status();
                let _ = self
                    .runtime_registry
                    .set_status(&target.runtime_id, status, err);
                // Refresh remote inventory snapshot.
                if let Err(err) = self.refresh_remote_inventory(&target.runtime_id) {
                    let (status, last_error) = crate::runtime::degraded_status(err);
                    let _ =
                        self.runtime_registry
                            .set_status(&target.runtime_id, status, last_error);
                }
                match self.runtime_registry.get(&target.runtime_id) {
                    Some(entry) => encode_success(
                        id,
                        ResponseResult::RuntimeConnected {
                            runtime: RuntimeInfo::from(entry),
                        },
                    ),
                    None => runtime_not_found(id, &target.runtime_id),
                }
            }
            Err(message) => {
                let (status, last_error) = offline_status(message.clone());
                let _ = self
                    .runtime_registry
                    .set_status(&target.runtime_id, status, last_error);
                self.remote_agent_cache.remove(&target.runtime_id);
                encode_error(id, "runtime_connect_failed", message)
            }
        }
    }

    pub(super) fn handle_runtime_disconnect(
        &mut self,
        id: String,
        target: RuntimeTarget,
    ) -> String {
        if target.runtime_id == crate::runtime::LOCAL_RUNTIME_ID {
            return encode_error(id, "invalid_runtime", "cannot disconnect the local runtime");
        }
        if !self.runtime_registry.contains(&target.runtime_id) {
            return runtime_not_found(id, &target.runtime_id);
        }
        let removed = self.runtime_connections.disconnect(&target.runtime_id);
        self.remote_agent_cache.remove(&target.runtime_id);
        let _ = self
            .runtime_registry
            .set_status(&target.runtime_id, RuntimeStatus::Offline, None);
        encode_success(
            id,
            ResponseResult::RuntimeDisconnected {
                runtime_id: target.runtime_id,
                disconnected: removed,
            },
        )
    }

    pub(crate) fn refresh_remote_inventory(&mut self, runtime_id: &str) -> Result<(), String> {
        let entry = self
            .runtime_registry
            .get(runtime_id)
            .cloned()
            .ok_or_else(|| format!("runtime '{runtime_id}' not found"))?;
        let connection = self
            .runtime_connections
            .get(runtime_id)
            .ok_or_else(|| format!("runtime '{runtime_id}' is not connected"))?;
        let agents = crate::runtime::fetch_remote_agents(connection, &entry)?;
        self.remote_agent_cache
            .insert(runtime_id.to_string(), agents);
        Ok(())
    }

    pub(crate) fn collect_aggregated_agent_infos(&mut self) -> Vec<crate::api::schema::AgentInfo> {
        let local = self.collect_agent_infos();
        let remote_ids: Vec<String> = self
            .runtime_registry
            .list()
            .iter()
            .filter(|entry| !entry.id.is_local())
            .filter(|entry| self.runtime_connections.is_connected(entry.id.as_str()))
            .map(|entry| entry.id.as_str().to_string())
            .collect();

        let mut remotes = Vec::new();
        for runtime_id in remote_ids {
            match self.refresh_remote_inventory(&runtime_id) {
                Ok(()) => {
                    if let Some(agents) = self.remote_agent_cache.get(&runtime_id) {
                        remotes.push(agents.clone());
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        runtime_id = %runtime_id,
                        error = %err,
                        "failed to refresh remote agent inventory"
                    );
                    let (status, last_error) = crate::runtime::degraded_status(err);
                    let _ = self
                        .runtime_registry
                        .set_status(&runtime_id, status, last_error);
                    // Keep last good snapshot if any.
                    if let Some(agents) = self.remote_agent_cache.get(&runtime_id) {
                        remotes.push(agents.clone());
                    }
                }
            }
        }
        let merged = crate::runtime::merge_agent_lists(local, remotes);
        // Project non-local rows into AppState for sidebar rendering.
        self.state.hub_remote_agents = merged
            .iter()
            .filter(|agent| agent.runtime_id != crate::runtime::LOCAL_RUNTIME_ID)
            .cloned()
            .collect();
        merged
    }

    pub(super) fn handle_runtime_get(&mut self, id: String, target: RuntimeTarget) -> String {
        match self.runtime_registry.get(&target.runtime_id) {
            Some(entry) => encode_success(
                id,
                ResponseResult::RuntimeInfo {
                    runtime: RuntimeInfo::from(entry),
                },
            ),
            None => runtime_not_found(id, &target.runtime_id),
        }
    }

    pub(super) fn handle_runtime_add(&mut self, id: String, params: RuntimeAddParams) -> String {
        if params.kind == RuntimeKind::Local {
            return encode_error(
                id,
                "invalid_runtime_kind",
                "cannot add a local runtime; it is always present",
            );
        }

        let runtime_id = match parse_runtime_id(&params.id) {
            Ok(runtime_id) => runtime_id,
            Err(message) => return encode_error(id, "invalid_runtime_id", message),
        };

        let label = params
            .label
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| runtime_id.as_str().to_string());

        let runtime_id_str = runtime_id.as_str().to_string();
        let entry = RuntimeEntry {
            id: runtime_id,
            kind: params.kind,
            label,
            target: params.target,
            session: params.session.filter(|value| !value.trim().is_empty()),
            status: RuntimeStatus::Offline,
            last_error: None,
        };

        if let Err(message) = self.runtime_registry.add(entry) {
            let code = if message.contains("already exists") {
                "runtime_already_exists"
            } else {
                "invalid_runtime"
            };
            return encode_error(id, code, message);
        }

        if let Err(err) = self.persist_runtime_registry() {
            // Best-effort rollback so memory matches disk.
            let _ = self.runtime_registry.remove(&runtime_id_str);
            return encode_error(id, "runtime_registry_save_failed", err.to_string());
        }

        let Some(entry) = self.runtime_registry.get(&runtime_id_str) else {
            return encode_error(
                id,
                "runtime_internal_error",
                "runtime missing after successful add",
            );
        };
        encode_success(
            id,
            ResponseResult::RuntimeAdded {
                runtime: RuntimeInfo::from(entry),
            },
        )
    }

    pub(super) fn handle_runtime_remove(&mut self, id: String, target: RuntimeTarget) -> String {
        if target.runtime_id == crate::runtime::LOCAL_RUNTIME_ID {
            return encode_error(
                id,
                "cannot_remove_local_runtime",
                "cannot remove the local runtime",
            );
        }
        if !self.runtime_registry.contains(&target.runtime_id) {
            return runtime_not_found(id, &target.runtime_id);
        }

        // Disconnect before remove so bridges are torn down.
        let _ = self.runtime_connections.disconnect(&target.runtime_id);
        self.remote_agent_cache.remove(&target.runtime_id);
        match self.runtime_registry.remove(&target.runtime_id) {
            Ok(entry) => {
                if let Err(err) = self.persist_runtime_registry() {
                    // Restore removed entry if save fails.
                    let _ = self.runtime_registry.add(entry);
                    return encode_error(id, "runtime_registry_save_failed", err.to_string());
                }
                encode_success(
                    id,
                    ResponseResult::RuntimeRemoved {
                        runtime_id: target.runtime_id,
                        removed: true,
                    },
                )
            }
            Err(message) => encode_error(id, "runtime_remove_failed", message),
        }
    }

    fn persist_runtime_registry(&self) -> std::io::Result<()> {
        if self.no_session {
            return Ok(());
        }
        let path = crate::runtime::registry_path();
        let membership = self.runtime_registry.membership_for_persist();
        crate::runtime::save_membership(&path, &membership)
    }
}

pub(crate) fn load_runtime_registry(no_session: bool) -> RuntimeRegistry {
    if no_session {
        return RuntimeRegistry::new();
    }
    let path = crate::runtime::registry_path();
    let membership = crate::runtime::load_membership(&path);
    match RuntimeRegistry::from_membership(membership) {
        Ok(registry) => registry,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                err = %err,
                "failed to restore runtime registry; using local-only"
            );
            RuntimeRegistry::new()
        }
    }
}

fn runtime_not_found(id: String, runtime_id: &str) -> String {
    encode_error(
        id,
        "runtime_not_found",
        format!("runtime '{runtime_id}' not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{Method, Request};
    use crate::app::App;
    use crate::config::Config;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    #[test]
    fn list_includes_local_connected() {
        let mut app = test_app();
        let response = app.handle_api_request(Request {
            id: "list".into(),
            method: Method::RuntimeList(crate::api::schema::EmptyParams::default()),
        });
        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert!(value.get("error").is_none(), "{response}");
        let runtimes = value["result"]["runtimes"].as_array().unwrap();
        assert_eq!(runtimes.len(), 1);
        assert_eq!(runtimes[0]["runtime_id"], "local");
        assert_eq!(runtimes[0]["kind"], "local");
        assert_eq!(runtimes[0]["status"], "connected");
    }

    #[test]
    fn add_get_remove_remote_offline() {
        let mut app = test_app();
        let add = app.handle_api_request(Request {
            id: "add".into(),
            method: Method::RuntimeAdd(RuntimeAddParams {
                id: "workbox".into(),
                kind: RuntimeKind::RemoteSsh,
                label: Some("Workbox".into()),
                target: Some("workbox.example".into()),
                session: Some("worker".into()),
            }),
        });
        let add_value: serde_json::Value = serde_json::from_str(&add).unwrap();
        assert!(add_value.get("error").is_none(), "{add}");
        assert_eq!(add_value["result"]["type"], "runtime_added");
        assert_eq!(add_value["result"]["runtime"]["runtime_id"], "workbox");
        assert_eq!(add_value["result"]["runtime"]["status"], "offline");
        assert_eq!(add_value["result"]["runtime"]["session"], "worker");

        let get = app.handle_api_request(Request {
            id: "get".into(),
            method: Method::RuntimeGet(RuntimeTarget {
                runtime_id: "workbox".into(),
            }),
        });
        let get_value: serde_json::Value = serde_json::from_str(&get).unwrap();
        assert_eq!(get_value["result"]["runtime"]["label"], "Workbox");

        let list = app.handle_api_request(Request {
            id: "list".into(),
            method: Method::RuntimeList(crate::api::schema::EmptyParams::default()),
        });
        let list_value: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(
            list_value["result"]["runtimes"].as_array().unwrap().len(),
            2
        );

        let remove = app.handle_api_request(Request {
            id: "remove".into(),
            method: Method::RuntimeRemove(RuntimeTarget {
                runtime_id: "workbox".into(),
            }),
        });
        let remove_value: serde_json::Value = serde_json::from_str(&remove).unwrap();
        assert!(remove_value.get("error").is_none(), "{remove}");
        assert_eq!(remove_value["result"]["removed"], true);

        let missing = app.handle_api_request(Request {
            id: "get2".into(),
            method: Method::RuntimeGet(RuntimeTarget {
                runtime_id: "workbox".into(),
            }),
        });
        let missing_value: serde_json::Value = serde_json::from_str(&missing).unwrap();
        assert_eq!(missing_value["error"]["code"], "runtime_not_found");
    }

    #[test]
    fn cannot_remove_local_or_add_duplicate() {
        let mut app = test_app();
        let remove_local = app.handle_api_request(Request {
            id: "rm".into(),
            method: Method::RuntimeRemove(RuntimeTarget {
                runtime_id: "local".into(),
            }),
        });
        let remove_value: serde_json::Value = serde_json::from_str(&remove_local).unwrap();
        assert_eq!(remove_value["error"]["code"], "cannot_remove_local_runtime");

        let params = RuntimeAddParams {
            id: "peer".into(),
            kind: RuntimeKind::Socket,
            label: None,
            target: Some("/tmp/peer.sock".into()),
            session: None,
        };
        let first = app.handle_api_request(Request {
            id: "a1".into(),
            method: Method::RuntimeAdd(params.clone()),
        });
        assert!(
            serde_json::from_str::<serde_json::Value>(&first)
                .unwrap()
                .get("error")
                .is_none(),
            "{first}"
        );
        let second = app.handle_api_request(Request {
            id: "a2".into(),
            method: Method::RuntimeAdd(params),
        });
        let second_value: serde_json::Value = serde_json::from_str(&second).unwrap();
        assert_eq!(second_value["error"]["code"], "runtime_already_exists");
    }
}
