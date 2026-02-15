use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

/// A log line captured from a managed proxy process.
pub struct LogLine {
    pub session_id: Uuid,
    pub level: String,
    pub message: String,
    pub timestamp: String,
}

pub struct ManagedProcess {
    pub pid: u32,
    pub child: Child,
    reader_handles: Vec<tokio::task::JoinHandle<()>>,
}

pub struct ProcessManager {
    processes: HashMap<Uuid, ManagedProcess>,
    proxy_path: PathBuf,
    backend_url: String,
    max_processes: usize,
    dev_mode: bool,
    log_tx: mpsc::UnboundedSender<LogLine>,
}

pub struct SpawnResult {
    pub session_id: Uuid,
    pub pid: u32,
}

impl ProcessManager {
    pub fn new(
        proxy_path: PathBuf,
        backend_url: String,
        max_processes: usize,
        dev_mode: bool,
    ) -> (Self, mpsc::UnboundedReceiver<LogLine>) {
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        (
            Self {
                processes: HashMap::new(),
                proxy_path,
                backend_url,
                max_processes,
                dev_mode,
                log_tx,
            },
            log_rx,
        )
    }

    pub fn running_session_ids(&self) -> Vec<Uuid> {
        self.processes.keys().copied().collect()
    }

    pub fn spawn(
        &mut self,
        auth_token: &str,
        working_directory: &str,
        session_name: Option<&str>,
        claude_args: &[String],
    ) -> anyhow::Result<SpawnResult> {
        if self.processes.len() >= self.max_processes {
            anyhow::bail!(
                "At process limit ({}/{})",
                self.processes.len(),
                self.max_processes
            );
        }

        let wd = std::path::Path::new(working_directory);
        if !wd.is_dir() {
            anyhow::bail!("Working directory does not exist: {}", working_directory);
        }

        let session_id = Uuid::new_v4();
        let default_name = format!("launched-{}", chrono::Local::now().format("%H%M%S"));
        let name = session_name.unwrap_or(&default_name);

        let mut cmd = tokio::process::Command::new(&self.proxy_path);
        cmd.arg("--backend-url").arg(&self.backend_url);
        cmd.arg("--session-name").arg(name);
        cmd.arg("--new-session");

        if self.dev_mode {
            cmd.arg("--dev");
        }

        cmd.env("PORTAL_AUTH_TOKEN", auth_token);
        cmd.arg("--auth-token").arg(auth_token);

        if !claude_args.is_empty() {
            cmd.arg("--");
            for arg in claude_args {
                cmd.arg(arg);
            }
        }

        cmd.current_dir(working_directory);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        // Take ownership of stdout/stderr and spawn reader tasks
        let mut reader_handles = Vec::new();

        if let Some(stdout) = child.stdout.take() {
            let tx = self.log_tx.clone();
            let sid = session_id;
            reader_handles.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let (level, message) = parse_log_line(&line);
                    let _ = tx.send(LogLine {
                        session_id: sid,
                        level,
                        message,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    });
                }
            }));
        }

        if let Some(stderr) = child.stderr.take() {
            let tx = self.log_tx.clone();
            let sid = session_id;
            reader_handles.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let (level, message) = parse_log_line(&line);
                    // Default stderr to warn if we couldn't parse a level
                    let level = if level == "info" && !line.contains("INFO") {
                        "warn".to_string()
                    } else {
                        level
                    };
                    let _ = tx.send(LogLine {
                        session_id: sid,
                        level,
                        message,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    });
                }
            }));
        }

        info!(
            "Spawned proxy process: pid={}, session_name={}, dir={}",
            pid, name, working_directory
        );

        let result = SpawnResult { session_id, pid };

        self.processes.insert(
            session_id,
            ManagedProcess {
                pid,
                child,
                reader_handles,
            },
        );

        Ok(result)
    }

    pub async fn stop(&mut self, session_id: &Uuid) -> bool {
        if let Some(mut proc) = self.processes.remove(session_id) {
            info!(
                "Stopping process for session {}, pid={}",
                session_id, proc.pid
            );
            if let Err(e) = proc.child.kill().await {
                warn!("Failed to kill process {}: {}", proc.pid, e);
            }
            for h in proc.reader_handles {
                h.abort();
            }
            true
        } else {
            warn!("No process found for session {}", session_id);
            false
        }
    }

    /// Check for exited child processes and remove them.
    /// Returns list of (session_id, exit_code) for processes that exited.
    pub fn reap_exited(&mut self) -> Vec<(Uuid, Option<i32>)> {
        let mut exited = Vec::new();

        for (session_id, proc) in self.processes.iter_mut() {
            match proc.child.try_wait() {
                Ok(Some(status)) => {
                    exited.push((*session_id, status.code()));
                }
                Ok(None) => { /* still running */ }
                Err(e) => {
                    error!("Error checking process {}: {}", proc.pid, e);
                    exited.push((*session_id, None));
                }
            }
        }

        for (session_id, code) in &exited {
            if let Some(proc) = self.processes.remove(session_id) {
                info!(
                    "Process exited: session={}, pid={}, code={:?}",
                    session_id, proc.pid, code
                );
                for h in proc.reader_handles {
                    h.abort();
                }
            }
        }

        exited
    }
}

/// Parse a tracing-format log line into (level, message).
/// Handles lines like: `2026-02-15T10:00:00Z  INFO proxy: Connected to backend`
/// Falls back to ("info", raw_line) if parsing fails.
fn parse_log_line(line: &str) -> (String, String) {
    let trimmed = line.trim();

    // Try to extract level from common tracing format:
    // "2026-02-15T... LEVEL module: message"
    for level in &["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] {
        if let Some(pos) = trimmed.find(level) {
            // Check it's a word boundary (preceded by whitespace)
            if pos > 0 && trimmed.as_bytes()[pos - 1] == b' ' {
                let after_level = &trimmed[pos + level.len()..];
                let message = after_level.trim_start_matches(' ').trim_start_matches(':');
                return (level.to_lowercase(), message.trim().to_string());
            }
        }
    }

    ("info".to_string(), trimmed.to_string())
}
