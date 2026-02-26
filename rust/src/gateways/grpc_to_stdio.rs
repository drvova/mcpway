use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::{FramedRead, LinesCodec};
use tonic::metadata::{MetadataKey, MetadataValue};
use tonic::transport::Endpoint;
use tonic::Request;
use url::Url;

use crate::grpc_proto::bridge::mcp_bridge_client::McpBridgeClient;
use crate::grpc_proto::bridge::Envelope;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::signals::install_signal_handlers;
use crate::transport::pool::{global_pool, transport_fingerprint};

const GRPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn run(
    endpoint: String,
    protocol_version: String,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    tracing::info!("  - grpc: {endpoint}");
    tracing::info!("Connecting to gRPC endpoint...");

    install_signal_handlers(None);

    let initial_runtime = runtime.get_effective(None).await;
    let warm_key = transport_fingerprint(
        "grpc",
        &endpoint,
        &initial_runtime.headers,
        &protocol_version,
    );

    let normalized = normalize_grpc_endpoint(&endpoint)?;
    let channel = Endpoint::from_shared(normalized)
        .map_err(|err| format!("Invalid gRPC endpoint {endpoint}: {err}"))?
        .connect_timeout(GRPC_CONNECT_TIMEOUT)
        .connect()
        .await
        .map_err(|err| format!("gRPC connection failed: {err}"))?;
    global_pool().mark_success(&warm_key, "grpc").await;

    let mut client = McpBridgeClient::new(channel);
    let (outbound_tx, outbound_rx) = mpsc::channel::<Envelope>(256);
    let mut request = Request::new(ReceiverStream::new(outbound_rx));
    apply_headers_to_metadata(request.metadata_mut(), &initial_runtime.headers)?;

    let mut inbound = client
        .stream(request)
        .await
        .map_err(|err| format!("gRPC stream failed: {err}"))?
        .into_inner();

    let runtime_store = runtime.clone();
    tokio::spawn(async move {
        while let Some(req) = updates.recv().await {
            let result = match req.update.scope {
                RuntimeScope::Global => {
                    let update_result = runtime_store.update_global(req.update.update).await;
                    if update_result.restart_needed || update_result.headers_changed {
                        RuntimeApplyResult::ok(
                            "Updated runtime args/headers; reconnect required for gRPC endpoint",
                            true,
                        )
                    } else {
                        RuntimeApplyResult::ok("No runtime changes applied", false)
                    }
                }
                RuntimeScope::Session(_) => RuntimeApplyResult::error(
                    "Per-session runtime overrides are not supported for gRPC→stdio",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let envelope_headers = Arc::new(initial_runtime.headers.clone());
    let seq = AtomicU64::new(0);
    let mut stdin_lines = FramedRead::new(tokio::io::stdin(), LinesCodec::new());

    loop {
        tokio::select! {
            line = tokio_stream::StreamExt::next(&mut stdin_lines) => {
                let Some(line) = line else {
                    break;
                };
                let line = line.map_err(|err| err.to_string())?;
                if line.trim().is_empty() {
                    continue;
                }

                let Ok(message_json) = serde_json::from_str::<serde_json::Value>(&line) else {
                    tracing::error!("Invalid JSON from stdin: {line}");
                    continue;
                };

                if !is_request(&message_json) {
                    println!("{}", message_json);
                    continue;
                }

                let envelope = Envelope {
                    json_rpc: message_json.to_string(),
                    metadata: envelope_headers.as_ref().clone(),
                    session_id: String::new(),
                    seq: seq.fetch_add(1, Ordering::SeqCst) + 1,
                };
                outbound_tx
                    .send(envelope)
                    .await
                    .map_err(|_| "gRPC outbound stream is closed".to_string())?;
            }
            frame = inbound.message() => {
                match frame {
                    Ok(Some(envelope)) => {
                        if envelope.json_rpc.trim().is_empty() {
                            continue;
                        }
                        let payload: serde_json::Value = serde_json::from_str(&envelope.json_rpc)
                            .map_err(|err| format!("gRPC envelope payload was not valid JSON: {err}"))?;
                        println!("{}", payload);
                    }
                    Ok(None) => return Err("gRPC connection closed".to_string()),
                    Err(err) => return Err(format!("gRPC stream error: {err}")),
                }
            }
        }
    }

    Ok(())
}

fn normalize_grpc_endpoint(endpoint: &str) -> Result<String, String> {
    if let Some(rest) = endpoint.strip_prefix("grpc://") {
        return Ok(format!("http://{rest}"));
    }
    if let Some(rest) = endpoint.strip_prefix("grpcs://") {
        return Ok(format!("https://{rest}"));
    }

    let url = Url::parse(endpoint).map_err(|err| format!("Invalid gRPC endpoint URL: {err}"))?;
    match url.scheme() {
        "http" | "https" => Ok(url.to_string()),
        other => Err(format!(
            "Unsupported gRPC endpoint scheme '{other}'. Use grpc://, grpcs://, http://, or https://"
        )),
    }
}

fn apply_headers_to_metadata(
    metadata: &mut tonic::metadata::MetadataMap,
    headers: &HashMap<String, String>,
) -> Result<(), String> {
    for (key, value) in headers {
        let lower = key.to_ascii_lowercase();
        let name = MetadataKey::from_bytes(lower.as_bytes())
            .map_err(|err| format!("Invalid metadata key '{key}': {err}"))?;
        let value = MetadataValue::try_from(value.as_str())
            .map_err(|err| format!("Invalid metadata value for '{key}': {err}"))?;
        metadata.insert(name, value);
    }
    Ok(())
}

fn is_request(message: &serde_json::Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}
