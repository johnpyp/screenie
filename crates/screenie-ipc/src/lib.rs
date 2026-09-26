//! How the `screenie` CLI talks to the resident daemon.
//!
//! The daemon listens on `$XDG_RUNTIME_DIR/screenie/<wayland-display>.sock`. The client
//! connects, and if nothing is listening it starts `screenie daemon` in the background and
//! retries, so users never manage the daemon by hand.
//!
//! Two locks beside the socket keep that to one daemon per session, however many
//! commands start at once: the daemon holds `<display>.lock` for as long as it runs, and
//! a client holds `<display>.spawn.lock` while it decides whether to start one.

mod lock;
mod protocol;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use lock::FileLock;
pub use protocol::*;
use screenie_config::Paths;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("daemon connection: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the daemon closed the connection without replying; see {log}")]
    Disconnected { log: PathBuf },
    #[error("another screenie daemon is already running")]
    AlreadyRunning,
    #[error("the daemon did not start; see {log}")]
    DaemonDidNotStart { log: PathBuf },
    #[error("the outdated daemon did not exit; run `screenie quit` and retry")]
    DaemonDidNotStop,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Write one message as a JSON line.
pub fn write_message<T: Serialize>(mut w: impl Write, msg: &T) -> Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()?;
    Ok(())
}

/// Read one JSON-line message. `Ok(None)` at end of stream.
pub fn read_message<T: DeserializeOwned>(r: &mut impl BufRead) -> Result<Option<T>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&line)?))
}

/// Identity of the running executable: its size and modification time. Any rebuild or
/// reinstall changes it, which is how a CLI notices the daemon runs an older binary. It
/// reads through `/proc/self/exe`, so it names the binary this process started from even
/// after that file was replaced on disk.
pub fn exe_stamp() -> String {
    static STAMP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STAMP
        .get_or_init(|| {
            let meta = std::fs::metadata("/proc/self/exe")
                .or_else(|_| std::fs::metadata(std::env::current_exe()?));
            match meta {
                Ok(m) => {
                    let mtime = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                    format!("{:x}-{:x}", mtime.map_or(0, |d| d.as_nanos()), m.len())
                }
                Err(_) => String::from("unknown"),
            }
        })
        .clone()
}

/// Path of the daemon's log file.
pub fn daemon_log_path() -> PathBuf {
    Paths::get().state_dir().join("daemon.log")
}

pub struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connect to a running daemon.
    pub fn connect() -> Result<Client> {
        Ok(Client {
            stream: UnixStream::connect(Paths::get().socket())?,
        })
    }

    /// Make way for a daemon running this executable: a daemon running another one (an
    /// upgrade, or a rebuild) is stopped, unless it's busy.
    pub fn take_over() -> Result<Takeover> {
        let Ok(client) = Self::connect() else {
            return Ok(Takeover::NotRunning);
        };
        let status = match client.request(&Request::Status)? {
            Response::Status(status) => *status,
            // Something answers, but not like a daemon: leave it be.
            _ => return Ok(Takeover::Busy(Status::default())),
        };
        if status.build == exe_stamp() {
            Ok(Takeover::Current(status))
        } else if status.busy() {
            Ok(Takeover::Busy(status))
        } else {
            tracing::info!(daemon = %status.commit, "replacing a daemon running another build");
            Self::replace_daemon()?;
            Ok(Takeover::Replaced(status))
        }
    }

    /// Connect, starting the daemon first if it isn't running. A daemon running another
    /// build is replaced first, unless it's busy, in which case it's used as-is and
    /// replaced on a later call.
    pub fn connect_or_spawn() -> Result<Client> {
        // One client at a time decides whether to start a daemon, so commands fired
        // together (at login, say) start one between them. The others wait here, then
        // find it running.
        let _deciding = FileLock::acquire(&spawn_lock_path(&Paths::get().socket()))?;
        match Self::take_over()? {
            Takeover::Current(_) | Takeover::Busy(_) => return Self::connect(),
            Takeover::NotRunning | Takeover::Replaced(_) => {}
        }
        spawn_daemon()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Self::connect() {
                Ok(client) => return Ok(client),
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => {
                    return Err(Error::DaemonDidNotStart {
                        log: daemon_log_path(),
                    });
                }
            }
        }
    }

    /// Ask the running daemon to quit, and wait until it has.
    fn replace_daemon() -> Result<()> {
        Self::connect()?.request(&Request::Quit)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Self::connect().is_ok() {
            if Instant::now() >= deadline {
                return Err(Error::DaemonDidNotStop);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    /// Send a request and wait (as long as it takes) for the response.
    pub fn request(mut self, request: &Request) -> Result<Response> {
        write_message(&mut self.stream, request)?;
        let mut reader = BufReader::new(self.stream);
        read_message(&mut reader)?.ok_or_else(|| Error::Disconnected {
            log: daemon_log_path(),
        })
    }

    /// Send [`Request::Watch`] and call `on_status` for every update until the daemon exits
    /// or `on_status` returns false.
    pub fn watch(mut self, mut on_status: impl FnMut(Status) -> bool) -> Result<()> {
        write_message(&mut self.stream, &Request::Watch)?;
        let mut reader = BufReader::new(self.stream);
        while let Some(response) = read_message::<Response>(&mut reader)? {
            if let Response::Status(status) = response
                && !on_status(*status)
            {
                break;
            }
        }
        Ok(())
    }
}

/// What [`Client::take_over`] found.
#[derive(Debug)]
pub enum Takeover {
    NotRunning,
    /// A daemon running this very executable.
    Current(Status),
    /// A daemon running another build, which has been stopped.
    Replaced(Status),
    /// A daemon running another build, left alone because it's in use (recording,
    /// selecting, editing).
    Busy(Status),
}

/// Start `screenie daemon` detached from this process's session, logging to the state
/// directory. The previous daemon's log is kept as `daemon.log.1`, so the next command
/// after a crash doesn't erase what the crash left.
fn spawn_daemon() -> Result<()> {
    let exe = std::env::current_exe()?;
    let log_path = daemon_log_path();
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::rename(&log_path, log_path.with_extension("log.1")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let log = std::fs::File::create(&log_path)?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        // It outlives the shell that started it: it mustn't keep that shell's directory
        // busy (an unmount would fail) or resolve anything against it.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut cmd);
    cmd.spawn()?;
    tracing::debug!("spawned daemon, logging to {}", log_path.display());
    Ok(())
}

fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe, which is all pre_exec requires.
    unsafe {
        cmd.pre_exec(|| {
            let _ = rustix::process::setsid();
            Ok(())
        });
    }
}

/// The lock a client holds while it decides whether to start a daemon.
fn spawn_lock_path(socket: &Path) -> PathBuf {
    socket.with_extension("spawn.lock")
}

/// The lock the daemon holds for as long as it runs.
fn daemon_lock_path(socket: &Path) -> PathBuf {
    socket.with_extension("lock")
}

/// A daemon's claim on its session's socket: while it's held, no other daemon can
/// listen there. Dropping it removes the socket, then lets the next daemon in.
#[derive(Debug)]
pub struct SocketClaim {
    socket: PathBuf,
    _lock: FileLock,
}

impl Drop for SocketClaim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Become the session's daemon: take the daemon lock, replace a stale socket file and
/// listen on it. Fails with [`Error::AlreadyRunning`] if another daemon has it.
pub fn bind_listener(path: &Path) -> Result<(UnixListener, SocketClaim)> {
    // A daemon that was just asked to quit may still be on its way out: give it a moment.
    claim_socket(path, Duration::from_secs(3))
}

fn claim_socket(path: &Path, patience: Duration) -> Result<(UnixListener, SocketClaim)> {
    let lock = FileLock::acquire_within(&daemon_lock_path(path), patience)?
        .ok_or(Error::AlreadyRunning)?;
    // A daemon from before the lock existed doesn't take it, but does answer.
    if UnixStream::connect(path).is_ok() {
        return Err(Error::AlreadyRunning);
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    Ok((
        listener,
        SocketClaim {
            socket: path.to_path_buf(),
            _lock: lock,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Daemons from before build stamps must read as "another build, idle", so a new
    /// CLI replaces them instead of talking to them forever.
    #[test]
    fn a_legacy_daemon_status_looks_outdated_and_idle() {
        let legacy = r#"{"type":"status","recording":null,"pid":7,"version":"0.1.0","compositor":"Hyprland","capture_backend":"wlr-screencopy-unstable-v1"}"#;
        let Response::Status(status) = serde_json::from_str(legacy).unwrap() else {
            panic!("not a status")
        };
        assert_ne!(status.build, exe_stamp());
        assert!(!status.busy());
    }

    /// A status bar started by an older binary must keep reading a newer daemon.
    #[test]
    fn a_newer_daemon_status_still_reads() {
        let newer = r#"{"type":"status","state":"rendering","recording":{"path":"/r.mp4","elapsed_secs":3.0,"paused":false,"fps":60},"editors":0,"sparkles":true}"#;
        let Response::Status(status) = serde_json::from_str(newer).unwrap() else {
            panic!("not a status")
        };
        assert_eq!(status.state, State::Unknown);
        assert_eq!(status.state.as_str(), "unknown");
        assert!(status.busy());
    }

    #[test]
    fn exe_stamp_is_stable_and_known() {
        assert_eq!(exe_stamp(), exe_stamp());
        assert_ne!(exe_stamp(), "unknown");
    }

    #[test]
    fn messages_roundtrip() {
        let req = Request::Screenshot(ScreenshotRequest {
            target: Target::Select {
                mode: SelectMode::Window,
            },
            delay: 2,
            actions: ActionOverrides {
                copy: Some(false),
                ..Default::default()
            },
            output: None,
            want_file: true,
            cursor: None,
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &req).unwrap();
        let back: Request = read_message(&mut buf.as_slice()).unwrap().unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn one_daemon_holds_the_socket_at_a_time() {
        let dir = std::env::temp_dir().join(format!("screenie-ipc-{}", std::process::id()));
        let socket = dir.join("wayland-test.sock");
        let (_listener, claim) = claim_socket(&socket, Duration::ZERO).unwrap();
        assert!(matches!(
            claim_socket(&socket, Duration::ZERO),
            Err(Error::AlreadyRunning)
        ));
        assert!(socket.exists(), "a failed claim leaves the socket alone");
        drop(claim);
        assert!(!socket.exists(), "the socket goes with its claim");
        let (_listener, _claim) = claim_socket(&socket, Duration::ZERO).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn eof_is_none() {
        assert!(read_message::<Request>(&mut &b""[..]).unwrap().is_none());
    }
}
