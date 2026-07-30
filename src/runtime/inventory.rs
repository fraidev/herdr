//! Fetch and merge agent inventory across hub runtimes.

use crate::api::schema::{AgentInfo, EmptyParams, Method, Request, ResponseResult};
use crate::runtime::connect::RuntimeConnection;
use crate::runtime::{scope_target, RuntimeEntry, RuntimeId, LOCAL_RUNTIME_ID};

/// Attach hub runtime identity to agent infos from a single runtime.
pub fn annotate_agents(
    agents: Vec<AgentInfo>,
    runtime_id: &str,
    runtime_label: Option<&str>,
) -> Vec<AgentInfo> {
    let is_local = runtime_id == LOCAL_RUNTIME_ID;
    let runtime = RuntimeId::new(runtime_id.to_string()).unwrap_or_else(|_| RuntimeId::local());
    agents
        .into_iter()
        .map(|mut agent| {
            agent.runtime_id = runtime_id.to_string();
            agent.runtime_label = if is_local {
                None
            } else {
                runtime_label.map(str::to_string)
            };
            if !is_local {
                // Scope public identity fields used as targets.
                if let Some(name) = agent.name.take() {
                    agent.name = Some(scope_target(&runtime, &name));
                }
                agent.pane_id = scope_target(&runtime, &agent.pane_id);
                agent.terminal_id = scope_target(&runtime, &agent.terminal_id);
                agent.workspace_id = scope_target(&runtime, &agent.workspace_id);
                agent.tab_id = scope_target(&runtime, &agent.tab_id);
            }
            agent
        })
        .collect()
}

/// Merge local agents with remote inventories (local first, then registry order).
pub fn merge_agent_lists(local: Vec<AgentInfo>, remotes: Vec<Vec<AgentInfo>>) -> Vec<AgentInfo> {
    let mut local = annotate_agents(local, LOCAL_RUNTIME_ID, None);
    for remote in remotes {
        local.extend(remote);
    }
    local
}

/// Fetch agent.list from a connected runtime and annotate with hub scope.
pub fn fetch_remote_agents(
    connection: &RuntimeConnection,
    entry: &RuntimeEntry,
) -> Result<Vec<AgentInfo>, String> {
    let response = connection.request(Request {
        id: format!("runtime:{}:agent.list", entry.id),
        method: Method::AgentList(EmptyParams::default()),
    })?;
    let agents = match response.result {
        ResponseResult::AgentList { agents } => agents,
        other => {
            return Err(format!(
                "unexpected agent.list result from {}: {other:?}",
                entry.id
            ))
        }
    };
    Ok(annotate_agents(
        agents,
        entry.id.as_str(),
        Some(entry.label.as_str()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;
    use std::collections::HashMap;

    fn sample_agent(name: &str, pane_id: &str) -> AgentInfo {
        AgentInfo {
            terminal_id: format!("term-{pane_id}"),
            name: Some(name.into()),
            agent: Some("claude".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: AgentStatus::Idle,
            screen_detection_skipped: false,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            agent_session: None,
            workspace_id: "w1".into(),
            tab_id: "t1".into(),
            pane_id: pane_id.into(),
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: 0,
            cwd: None,
            foreground_cwd: None,
            revision: 1,
            runtime_id: LOCAL_RUNTIME_ID.into(),
            runtime_label: None,
        }
    }

    #[test]
    fn annotate_local_leaves_ids_bare() {
        let agents = annotate_agents(
            vec![sample_agent("reviewer", "w1:p1")],
            LOCAL_RUNTIME_ID,
            None,
        );
        assert_eq!(agents[0].runtime_id, "local");
        assert_eq!(agents[0].name.as_deref(), Some("reviewer"));
        assert_eq!(agents[0].pane_id, "w1:p1");
        assert!(agents[0].runtime_label.is_none());
    }

    #[test]
    fn annotate_remote_scopes_targets() {
        let agents = annotate_agents(
            vec![sample_agent("reviewer", "w1:p1")],
            "workbox",
            Some("Workbox"),
        );
        assert_eq!(agents[0].runtime_id, "workbox");
        assert_eq!(agents[0].runtime_label.as_deref(), Some("Workbox"));
        assert_eq!(agents[0].name.as_deref(), Some("workbox/reviewer"));
        assert_eq!(agents[0].pane_id, "workbox/w1:p1");
        assert_eq!(agents[0].terminal_id, "workbox/term-w1:p1");
    }

    #[test]
    fn merge_local_then_remotes() {
        let local = vec![sample_agent("local-a", "p1")];
        let remote = annotate_agents(
            vec![sample_agent("remote-a", "p2")],
            "workbox",
            Some("Workbox"),
        );
        let merged = merge_agent_lists(local, vec![remote]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].runtime_id, "local");
        assert_eq!(merged[1].runtime_id, "workbox");
        assert_eq!(merged[1].name.as_deref(), Some("workbox/remote-a"));
    }
}
