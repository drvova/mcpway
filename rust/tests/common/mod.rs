#![allow(dead_code)]

use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

pub fn find_free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("failed to bind local test port")
        .local_addr()
        .expect("failed to read local address")
        .port()
}

pub async fn spawn_mcpway(args: &[&str], pipe_stdin: bool, pipe_stdout: bool) -> Child {
    let exe = mcpway_exe_path();

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_remove("PORT")
        .stdin(if pipe_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if pipe_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .kill_on_drop(true);

    cmd.spawn().expect("failed to spawn mcpway")
}

fn mcpway_exe_path() -> PathBuf {
    if let Ok(exe) = std::env::var("CARGO_BIN_EXE_mcpway") {
        return PathBuf::from(exe);
    }

    let current = std::env::current_exe().expect("failed to read current test binary path");
    let target_dir = current
        .parent()
        .and_then(|path| path.parent())
        .expect("failed to derive target/debug directory from test binary path");
    let fallback = target_dir.join(format!("mcpway{}", std::env::consts::EXE_SUFFIX));
    assert!(
        fallback.exists(),
        "mcpway binary not found at {}",
        fallback.display()
    );
    fallback
}

pub async fn stop_child(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

pub async fn wait_for_http_status(url: &str, expected: reqwest::StatusCode, timeout: Duration) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(response) = client.get(url).send().await {
            if response.status() == expected {
                return;
            }
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for {url} to return status {expected}"
        );

        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn wait_for_condition<F, Fut>(timeout: Duration, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if condition().await {
            return;
        }

        assert!(Instant::now() < deadline, "timed out waiting for condition");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn initialize_request(id: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "integration-test",
                "version": "0.1.0"
            }
        }
    })
}
