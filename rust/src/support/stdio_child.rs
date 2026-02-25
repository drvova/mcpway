use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, Mutex};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::types::RuntimeArgs;

// Keep enough headroom for bursty integration/CI traffic before consumers drain.
const CHILD_BROADCAST_BUFFER: usize = 2048;

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn build_command(&self, runtime: &RuntimeArgs) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        if !runtime.extra_cli_args.is_empty() {
            cmd.args(&runtime.extra_cli_args);
        }
        if !runtime.env.is_empty() {
            cmd.envs(runtime.env.clone());
        }
        cmd
    }
}

pub struct StdioChild {
    spec: CommandSpec,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    sender: broadcast::Sender<serde_json::Value>,
    restarting: Arc<AtomicBool>,
    exit_on_close: bool,
}

impl StdioChild {
    pub fn new(spec: CommandSpec, exit_on_close: bool) -> Self {
        let (sender, _) = broadcast::channel(CHILD_BROADCAST_BUFFER);
        Self {
            spec,
            stdin: Mutex::new(None),
            child: Mutex::new(None),
            sender,
            restarting: Arc::new(AtomicBool::new(false)),
            exit_on_close,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<serde_json::Value> {
        self.sender.subscribe()
    }

    pub async fn spawn(&self, runtime: &RuntimeArgs) -> Result<(), String> {
        let mut cmd = self.spec.build_command(runtime);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|err| err.to_string())?;
        let stdin = child.stdin.take().ok_or("Missing child stdin")?;
        let stdout = child.stdout.take().ok_or("Missing child stdout")?;
        let stderr = child.stderr.take().ok_or("Missing child stderr")?;

        {
            let mut guard = self.stdin.lock().await;
            *guard = Some(stdin);
        }
        {
            let mut guard = self.child.lock().await;
            *guard = Some(child);
        }

        let sender = self.sender.clone();
        let restarting = self.restarting.clone();
        let exit_on_close = self.exit_on_close;
        tokio::spawn(async move {
            let mut lines = FramedRead::new(stdout, LinesCodec::new());
            while let Some(line) = lines.next().await {
                match line {
                    Ok(line) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&line) {
                            Ok(json) => {
                                tracing::debug!("Child → Gateway: {json}");
                                let _ = sender.send(json);
                            }
                            Err(_) => {
                                tracing::error!("Child non-JSON: {line}");
                            }
                        }
                    }
                    Err(err) => {
                        tracing::error!("Error reading child stdout: {err}");
                        break;
                    }
                }
            }
            if exit_on_close && !restarting.load(Ordering::SeqCst) {
                tracing::error!("Child stdout closed. Exiting...");
                std::process::exit(1);
            }
        });

        tokio::spawn(async move {
            let mut lines = FramedRead::new(stderr, LinesCodec::new());
            while let Some(line) = lines.next().await {
                match line {
                    Ok(line) => {
                        if !line.trim().is_empty() {
                            tracing::error!("Child stderr: {line}");
                        }
                    }
                    Err(err) => {
                        tracing::error!("Error reading child stderr: {err}");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn send(&self, message: &serde_json::Value) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|err| err.to_string())?;
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or("Child stdin not available")?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    pub async fn is_alive(&self) -> bool {
        let should_clear = {
            let mut guard = self.child.lock().await;
            let Some(child) = guard.as_mut() else {
                return false;
            };
            match child.try_wait() {
                Ok(None) => return true,
                Ok(Some(_status)) => {
                    *guard = None;
                    true
                }
                Err(err) => {
                    tracing::error!("Failed to poll child status: {err}");
                    *guard = None;
                    true
                }
            }
        };
        if should_clear {
            let mut stdin = self.stdin.lock().await;
            *stdin = None;
        }
        false
    }

    pub async fn restart(&self, runtime: &RuntimeArgs) -> Result<(), String> {
        self.restarting.store(true, Ordering::SeqCst);
        {
            let mut guard = self.child.lock().await;
            if let Some(child) = guard.as_mut() {
                let _ = child.kill().await;
            }
            *guard = None;
        }
        {
            let mut stdin = self.stdin.lock().await;
            *stdin = None;
        }
        let result = self.spawn(runtime).await;
        self.restarting.store(false, Ordering::SeqCst);
        result
    }

    pub async fn shutdown(&self) {
        self.restarting.store(true, Ordering::SeqCst);
        {
            let mut guard = self.child.lock().await;
            if let Some(child) = guard.as_mut() {
                let _ = child.kill().await;
            }
            *guard = None;
        }
        {
            let mut stdin = self.stdin.lock().await;
            *stdin = None;
        }
        self.restarting.store(false, Ordering::SeqCst);
    }
}
