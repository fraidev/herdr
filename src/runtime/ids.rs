//! Runtime id validation and scoped public targets.
//!
//! Local agents keep bare targets (`pane_id` / name). Remote agents use
//! `{runtime_id}/{existing_target}`.

use super::{RuntimeId, LOCAL_RUNTIME_ID};

const MAX_RUNTIME_ID_LEN: usize = 64;

/// Validate a runtime id for registry membership.
///
/// Accepts ASCII letters, numbers, `.`, `_`, and `-`. Rejects empty values,
/// path separators, and the reserved `local` id (callers that need the local
/// entry construct it via [`RuntimeId::local`]).
pub fn validate_runtime_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("runtime id cannot be empty".into());
    }
    if id.len() > MAX_RUNTIME_ID_LEN {
        return Err(format!(
            "runtime id cannot be longer than {MAX_RUNTIME_ID_LEN} bytes"
        ));
    }
    if id == LOCAL_RUNTIME_ID {
        return Err("runtime id 'local' is reserved".into());
    }
    if id == "." || id == ".." {
        return Err("runtime id cannot be . or ..".into());
    }
    if id.contains('/') {
        return Err("runtime id must not contain '/'".into());
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("runtime id may only contain ASCII letters, numbers, '.', '_' and '-'".into());
    }
    Ok(())
}

/// Parse and validate a non-local runtime id.
pub fn parse_runtime_id(id: &str) -> Result<RuntimeId, String> {
    validate_runtime_id(id)?;
    RuntimeId::new(id.to_string()).map_err(|()| "runtime id cannot be empty".into())
}

/// Scope a public target for a runtime.
///
/// Local targets stay bare for backward compatibility. Non-local targets are
/// prefixed with `{runtime_id}/`.
///
/// Inventory merge (PR3+) will use this for remote agent ids.
#[allow(dead_code)]
pub fn scope_target(runtime_id: &RuntimeId, target: &str) -> String {
    if runtime_id.is_local() {
        target.to_string()
    } else {
        format!("{}/{target}", runtime_id.as_str())
    }
}

/// Split a possibly scoped public target into `(runtime_id, local_target)`.
///
/// When there is no `/`, the target is treated as local. When the first
/// segment is `local`, it is also treated as local (the prefix is stripped).
///
/// Returns `None` when a remote-style prefix is empty or the local half is empty.
///
/// Target resolution (PR3+) will use this for scoped agent/pane targets.
#[allow(dead_code)]
pub fn unscope_target(scoped: &str) -> Option<(RuntimeId, &str)> {
    let Some((prefix, rest)) = scoped.split_once('/') else {
        return Some((RuntimeId::local(), scoped));
    };
    if prefix.is_empty() || rest.is_empty() {
        return None;
    }
    if prefix == LOCAL_RUNTIME_ID {
        return Some((RuntimeId::local(), rest));
    }
    let runtime_id = RuntimeId::new(prefix.to_string()).ok()?;
    Some((runtime_id, rest))
}

/// Whether `scoped` uses a non-local runtime prefix.
#[allow(dead_code)]
pub fn is_scoped_remote_target(scoped: &str) -> bool {
    match unscope_target(scoped) {
        Some((runtime_id, _)) => !runtime_id.is_local(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_reserved_and_invalid_ids() {
        assert!(validate_runtime_id("local").is_err());
        assert!(validate_runtime_id("").is_err());
        assert!(validate_runtime_id("has/slash").is_err());
        assert!(validate_runtime_id("bad id").is_err());
        assert!(validate_runtime_id("workbox").is_ok());
        assert!(validate_runtime_id("worker-1").is_ok());
        assert!(validate_runtime_id("a.b_c-1").is_ok());
    }

    #[test]
    fn scope_local_stays_bare() {
        assert_eq!(scope_target(&RuntimeId::local(), "reviewer"), "reviewer");
        assert_eq!(scope_target(&RuntimeId::local(), "w1:p2"), "w1:p2");
    }

    #[test]
    fn scope_remote_prefixes_id() {
        let id = RuntimeId::new("workbox".to_string()).unwrap();
        assert_eq!(scope_target(&id, "reviewer"), "workbox/reviewer");
        assert_eq!(scope_target(&id, "w1:p2"), "workbox/w1:p2");
    }

    #[test]
    fn unscope_bare_is_local() {
        let (id, target) = unscope_target("reviewer").unwrap();
        assert!(id.is_local());
        assert_eq!(target, "reviewer");
    }

    #[test]
    fn unscope_remote_and_explicit_local() {
        let (id, target) = unscope_target("workbox/reviewer").unwrap();
        assert_eq!(id.as_str(), "workbox");
        assert_eq!(target, "reviewer");

        let (id, target) = unscope_target("local/w1:p2").unwrap();
        assert!(id.is_local());
        assert_eq!(target, "w1:p2");
    }

    #[test]
    fn unscope_rejects_empty_halves() {
        assert!(unscope_target("/reviewer").is_none());
        assert!(unscope_target("workbox/").is_none());
    }

    #[test]
    fn is_scoped_remote_detects_prefix() {
        assert!(!is_scoped_remote_target("reviewer"));
        assert!(!is_scoped_remote_target("local/reviewer"));
        assert!(is_scoped_remote_target("workbox/reviewer"));
    }
}
