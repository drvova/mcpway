mod common;

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::{initialize_request, spawn_mcpway, stop_child};

#[tokio::test]
async fn stdio_to_stdio_roundtrip_smoke() {
    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--output-transport",
            "stdio",
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let request = initialize_request("stdio-stdio-init");
    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("failed to write initialize request to stdin");
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("timed out waiting for stdout response")
        .expect("failed reading stdout line")
        .expect("stdout closed before response line");

    let payload: serde_json::Value =
        serde_json::from_str(&line).expect("stdout line was not valid JSON");
    assert_eq!(
        payload.get("id"),
        Some(&serde_json::json!("stdio-stdio-init"))
    );
    assert_eq!(
        payload.get("method"),
        Some(&serde_json::json!("initialize"))
    );

    stop_child(&mut child).await;
}

#[tokio::test]
async fn stdio_to_stdio_logs_invalid_json_and_continues() {
    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--output-transport",
            "stdio",
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let stdin = child.stdin.as_mut().expect("stdin was not piped");
    stdin
        .write_all(b"this-is-not-json\n")
        .await
        .expect("failed to write malformed input");

    let request = initialize_request("stdio-stdio-after-bad-input");
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("failed to write initialize request to stdin");
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = child.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("timed out waiting for stdout response")
        .expect("failed reading stdout line")
        .expect("stdout closed before response line");

    let payload: serde_json::Value =
        serde_json::from_str(&line).expect("stdout line was not valid JSON");
    assert_eq!(
        payload.get("id"),
        Some(&serde_json::json!("stdio-stdio-after-bad-input"))
    );
    assert_eq!(
        payload.get("method"),
        Some(&serde_json::json!("initialize"))
    );

    stop_child(&mut child).await;
}
