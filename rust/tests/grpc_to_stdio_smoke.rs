mod common;

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::{find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_condition};

#[tokio::test]
async fn grpc_to_stdio_roundtrip_smoke() {
    let upstream_port = find_free_port();
    let upstream_port_str = upstream_port.to_string();

    let mut upstream = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--output-transport",
            "grpc",
            "--port",
            &upstream_port_str,
            "--log-level",
            "none",
        ],
        false,
        false,
    )
    .await;

    wait_for_condition(Duration::from_secs(10), || async {
        let Ok(channel) =
            tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{upstream_port}"))
                .expect("valid endpoint")
                .connect()
                .await
        else {
            return false;
        };
        let mut client =
            mcpway::grpc_proto::bridge::mcp_bridge_client::McpBridgeClient::new(channel);
        client
            .health(tonic::Request::new(
                mcpway::grpc_proto::bridge::HealthRequest {},
            ))
            .await
            .is_ok()
    })
    .await;

    let mut bridge = spawn_mcpway(
        &[
            "connect",
            &format!("grpc://127.0.0.1:{upstream_port}"),
            "--protocol",
            "grpc",
            "--log-level",
            "none",
        ],
        true,
        true,
    )
    .await;

    let initialize = initialize_request("grpc-inbound-init");
    let stdin = bridge.stdin.as_mut().expect("stdin was not piped");
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .expect("failed to write initialize request to stdin");
    stdin.flush().await.expect("failed to flush stdin");

    let stdout = bridge.stdout.take().expect("stdout was not piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("timed out waiting for stdout response")
        .expect("failed reading stdout line")
        .expect("stdout closed before response line");

    let payload: serde_json::Value =
        serde_json::from_str(&line).expect("stdout line was not valid JSON");
    assert_eq!(
        payload.get("id"),
        Some(&serde_json::json!("grpc-inbound-init"))
    );
    assert_eq!(
        payload.get("method"),
        Some(&serde_json::json!("initialize"))
    );

    stop_child(&mut bridge).await;
    stop_child(&mut upstream).await;
}
