//! Live connections from the hub to remote/socket runtimes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::api::client::{ApiClient, ConnectionTarget};
use crate::api::schema::{Method, Request};
use crate::runtime::{RuntimeEntry, RuntimeKind, RuntimeStatus};

const CONNECT_PING_TIMEOUT: Duration = Duration::from_secs(3);

/// Live connection to a non-local runtime.
pub struct RuntimeConnection {
    pub runtime_id: String,
    pub client: ApiClient,
    /// Keep SSH stdio bridge alive for the connection lifetime.
    #[cfg(unix)]
    _bridge: Option<ApiSshBridge>,
}

impl RuntimeConnection {
    pub fn ping(&self) -> Result<(), String> {
        self.client
            .request_value_with_timeout(
                &Request {
                    id: format!("runtime:{}:ping", self.runtime_id),
                    method: Method::Ping(crate::api::schema::PingParams::default()),
                },
                CONNECT_PING_TIMEOUT,
            )
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    pub fn request(&self, request: Request) -> Result<crate::api::schema::SuccessResponse, String> {
        self.client.request(request).map_err(|err| err.to_string())
    }
}

/// Map of connected runtime ids → live API clients / bridges.
#[derive(Default)]
pub struct RuntimeConnections {
    connections: HashMap<String, RuntimeConnection>,
}

impl RuntimeConnections {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, runtime_id: &str) -> Option<&RuntimeConnection> {
        self.connections.get(runtime_id)
    }

    pub fn is_connected(&self, runtime_id: &str) -> bool {
        self.connections.contains_key(runtime_id)
    }

    pub fn disconnect(&mut self, runtime_id: &str) -> bool {
        self.connections.remove(runtime_id).is_some()
    }

    /// Connect to a registered non-local runtime and verify with `ping`.
    pub fn connect(&mut self, entry: &RuntimeEntry) -> Result<&RuntimeConnection, String> {
        if entry.kind == RuntimeKind::Local || entry.id.is_local() {
            return Err("local runtime is always connected".into());
        }
        let runtime_id = entry.id.as_str().to_string();
        // Replace any existing connection for this id.
        self.connections.remove(&runtime_id);

        let connection = match entry.kind {
            RuntimeKind::Local => return Err("local runtime is always connected".into()),
            RuntimeKind::Socket => connect_socket(entry)?,
            RuntimeKind::RemoteSsh => connect_remote_ssh(entry)?,
        };

        if let Err(err) = connection.ping() {
            // Drop bridge/socket on failed health check.
            drop(connection);
            return Err(format!("connected but ping failed: {err}"));
        }

        self.connections.insert(runtime_id.clone(), connection);
        self.connections
            .get(&runtime_id)
            .ok_or_else(|| "connection missing after insert".to_string())
    }
}

fn connect_socket(entry: &RuntimeEntry) -> Result<RuntimeConnection, String> {
    let path = entry
        .target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "socket runtime requires a target path".to_string())?;
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("socket path does not exist: {}", path.display()));
    }
    Ok(RuntimeConnection {
        runtime_id: entry.id.as_str().to_string(),
        client: ApiClient::for_target(ConnectionTarget::SocketPath(path)),
        #[cfg(unix)]
        _bridge: None,
    })
}

#[cfg(unix)]
fn connect_remote_ssh(entry: &RuntimeEntry) -> Result<RuntimeConnection, String> {
    let target = entry
        .target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "remote_ssh runtime requires an SSH host target".to_string())?;
    let session = entry
        .session
        .as_deref()
        .filter(|value| !value.trim().is_empty());

    let local_socket = api_bridge_socket_path(entry.id.as_str())?;
    let bridge = ApiSshBridge::start(
        target.to_string(),
        session.map(str::to_string),
        local_socket.clone(),
    )
    .map_err(|err| format!("failed to start SSH API bridge: {err}"))?;

    // Give the bridge a moment to bind before first connect.
    std::thread::sleep(Duration::from_millis(50));

    Ok(RuntimeConnection {
        runtime_id: entry.id.as_str().to_string(),
        client: ApiClient::for_target(ConnectionTarget::SocketPath(local_socket)),
        _bridge: Some(bridge),
    })
}

#[cfg(not(unix))]
fn connect_remote_ssh(_entry: &RuntimeEntry) -> Result<RuntimeConnection, String> {
    Err("remote_ssh runtimes are not supported on this platform".into())
}

fn api_bridge_socket_path(runtime_id: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "herdr-runtime-{}-{}-api.sock",
        sanitize_id(runtime_id),
        std::process::id()
    ));
    Ok(path)
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Map a successful connect onto registry status fields.
pub fn connected_status() -> (RuntimeStatus, Option<String>) {
    (RuntimeStatus::Connected, None)
}

pub fn offline_status(error: impl Into<String>) -> (RuntimeStatus, Option<String>) {
    (RuntimeStatus::Offline, Some(error.into()))
}

pub fn degraded_status(error: impl Into<String>) -> (RuntimeStatus, Option<String>) {
    (RuntimeStatus::Degraded, Some(error.into()))
}

#[cfg(unix)]
struct ApiSshBridge {
    local_socket: PathBuf,
    should_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl ApiSshBridge {
    fn start(
        ssh_target: String,
        session: Option<String>,
        local_socket: PathBuf,
    ) -> std::io::Result<Self> {
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let _ = std::fs::remove_file(&local_socket);
        let listener = UnixListener::bind(&local_socket)?;
        crate::ipc::restrict_socket_permissions(&local_socket, 0o600)?;
        listener.set_nonblocking(true)?;

        let should_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&should_stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Err(err) = stream.set_nonblocking(false) {
                            tracing::warn!(error = %err, "runtime api bridge accept prepare failed");
                            continue;
                        }
                        if let Err(err) =
                            bridge_api_connection(stream, &ssh_target, session.as_deref())
                        {
                            tracing::warn!(error = %err, "runtime api bridge session failed");
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "runtime api bridge listener failed");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            local_socket,
            should_stop,
            thread: Some(thread),
        })
    }
}

#[cfg(unix)]
impl Drop for ApiSshBridge {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.should_stop.store(true, Ordering::Release);
        let _ = std::fs::remove_file(&self.local_socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn bridge_api_connection(
    stream: std::os::unix::net::UnixStream,
    ssh_target: &str,
    session: Option<&str>,
) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    use std::thread;

    let remote_cmd = remote_api_bridge_command(session);
    let mut command = Command::new("ssh");
    command
        .arg("-T")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=4")
        .arg(ssh_target)
        .arg(remote_cmd);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = command.spawn().map_err(|err| {
        std::io::Error::new(err.kind(), format!("failed to start ssh api bridge: {err}"))
    })?;
    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ssh stdin missing"))?;
    let mut child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ssh stdout missing"))?;
    let mut stream_to_child = stream.try_clone()?;
    let mut child_to_stream = stream;

    let upload = thread::spawn(move || {
        let _ = copy_flush(&mut stream_to_child, &mut child_stdin);
    });
    let download = thread::spawn(move || {
        let _ = copy_flush(&mut child_stdout, &mut child_to_stream);
        let _ = child_to_stream.shutdown(std::net::Shutdown::Write);
    });

    let status = child.wait()?;
    let _ = upload.join();
    let _ = download.join();
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            format!("ssh api bridge exited with {status}"),
        ))
    }
}

#[cfg(unix)]
fn remote_api_bridge_command(session: Option<&str>) -> String {
    // Prefer PATH `herdr` on the remote; users with custom installs can set HERDR_REMOTE_BINARY later.
    let mut command = String::from("exec herdr");
    if let Some(session) = session {
        if session != crate::session::DEFAULT_SESSION_NAME {
            command.push_str(" --session ");
            command.push_str(&shell_quote(session));
        }
    }
    command.push_str(" remote-api-bridge");
    command
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
                )
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn copy_flush<R: std::io::Read, W: std::io::Write>(
    reader: &mut R,
    writer: &mut W,
) -> std::io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0;
    loop {
        let bytes_read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        writer.write_all(&buffer[..bytes_read])?;
        writer.flush()?;
        total += bytes_read as u64;
    }
}

/// Run remote-api-bridge: connect to remote herdr.sock and copy stdio.
#[cfg(unix)]
pub fn run_remote_api_bridge() -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    use std::thread;

    let socket_path = crate::session::active_api_socket_path();
    let stream = UnixStream::connect(&socket_path).map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!(
                "failed to connect to remote Herdr API socket {}: {err}",
                socket_path.display()
            ),
        )
    })?;

    let mut stdout = std::io::stdout().lock();
    let mut socket_to_stdout = stream.try_clone()?;
    let mut stdin_to_socket = stream;

    let _upload = thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let _ = copy_flush(&mut stdin, &mut stdin_to_socket);
        let _ = stdin_to_socket.shutdown(std::net::Shutdown::Write);
    });

    copy_flush(&mut socket_to_stdout, &mut stdout).map(|_| ())
}

#[cfg(not(unix))]
pub fn run_remote_api_bridge() -> std::io::Result<()> {
    Err(std::io::Error::other(
        "remote-api-bridge is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RuntimeId, RuntimeKind, RuntimeStatus};

    #[test]
    fn connect_socket_missing_path_errors() {
        let entry = RuntimeEntry {
            id: RuntimeId::new("peer".to_string()).unwrap(),
            kind: RuntimeKind::Socket,
            label: "Peer".into(),
            target: Some("/tmp/herdr-definitely-missing-socket-xyz.sock".into()),
            session: None,
            status: RuntimeStatus::Offline,
            last_error: None,
        };
        let mut connections = RuntimeConnections::new();
        assert!(connections.connect(&entry).is_err());
    }

    #[test]
    fn sanitize_id_strips_unsafe() {
        assert_eq!(sanitize_id("workbox"), "workbox");
        assert_eq!(sanitize_id("a/b c"), "a_b_c");
    }
}
