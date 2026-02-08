use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Child;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct ManagedProcess {
    pub pid: u32,
    pub child: Child,
}

pub struct ProcessManager {
    processes: HashMap<Uuid, ManagedProcess>,
    proxy_path: PathBuf,
    backend_url: String,
    max_processes: usize,
    dev_mode: bool,
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
    ) -> Self {
        Self {
            processes: HashMap::new(),
            proxy_path,
            backend_url,
            max_processes,
            dev_mode,
        }
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

        // Validate working directory exists
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

        // Pass auth token via env var to avoid /proc exposure
        cmd.env("PORTAL_AUTH_TOKEN", auth_token);
        cmd.arg("--auth-token").arg(auth_token);

        // Pass through extra claude args after --
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

        let child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        info!(
            "Spawned proxy process: pid={}, session_name={}, dir={}",
            pid, name, working_directory
        );

        let result = SpawnResult { session_id, pid };

        self.processes
            .insert(session_id, ManagedProcess { pid, child });

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
            }
        }

        exited
    }
}
