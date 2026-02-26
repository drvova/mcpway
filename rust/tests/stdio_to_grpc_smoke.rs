mod common;

use std::time::Duration;

use mcpway::grpc_proto::bridge::mcp_bridge_client::McpBridgeClient;
use mcpway::grpc_proto::bridge::{Envelope, HealthRequest};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Endpoint;
use tonic::Request;

use common::{find_free_port, initialize_request, spawn_mcpway, stop_child, wait_for_condition};

#[tokio::test]
async fn stdio_to_grpc_roundtrip_smoke() {
    let port = find_free_port();
    let port_str = port.to_string();

    let mut child = spawn_mcpway(
        &[
            "--stdio",
            "cat",
            "--output-transport",
            "grpc",
            "--port",
            &port_str,
            "--log-level",
            "none",
        ],
        false,
        false,
    )
    .await;

    wait_for_condition(Duration::from_secs(10), || async {
        let Ok(channel) = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .expect("valid endpoint")
            .connect()
            .await
        else {
            return false;
        };
        let mut client = McpBridgeClient::new(channel);
        client.health(Request::new(HealthRequest {})).await.is_ok()
    })
    .await;

    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("valid endpoint")
        .connect()
        .await
        .expect("connect to grpc endpoint");
    let mut client = McpBridgeClient::new(channel);
    let (tx, rx) = mpsc::channel::<Envelope>(8);

    let mut inbound = client
        .stream(Request::new(ReceiverStream::new(rx)))
        .await
        .expect("open grpc stream")
        .into_inner();

    let initialize = initialize_request("grpc-init");
    tx.send(Envelope {
        json_rpc: initialize.to_string(),
        metadata: Default::default(),
        session_id: String::new(),
        seq: 1,
    })
    .await
    .expect("send initialize message");

    let response = tokio::time::timeout(Duration::from_secs(5), inbound.message())
        .await
        .expect("timed out waiting for grpc response")
        .expect("grpc stream returned error")
        .expect("grpc stream closed");

    let payload: serde_json::Value =
        serde_json::from_str(&response.json_rpc).expect("grpc payload was not valid JSON");
    assert_eq!(payload.get("id"), Some(&serde_json::json!("grpc-init")));
    assert_eq!(
        payload.get("method"),
        Some(&serde_json::json!("initialize"))
    );

    stop_child(&mut child).await;
}
