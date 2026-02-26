use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use futures::Stream;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::config::Config;
use crate::grpc_proto::bridge::mcp_bridge_server::{McpBridge, McpBridgeServer};
use crate::grpc_proto::bridge::{Envelope, HealthRequest, HealthResponse};
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::command_spec::parse_command_spec;
use crate::support::signals::install_signal_handlers;
use crate::support::stdio_child::StdioChild;

const GRPC_CLIENT_BUFFER: usize = 256;

type GrpcEnvelopeSender = mpsc::Sender<Result<Envelope, Status>>;
type GrpcClientMap = Arc<Mutex<HashMap<String, GrpcEnvelopeSender>>>;

#[derive(Clone)]
struct AppState {
    clients: GrpcClientMap,
    child: Arc<StdioChild>,
    seq: Arc<AtomicU64>,
    bearer_token: Option<String>,
}

#[derive(Clone)]
struct BridgeService {
    state: AppState,
}

#[tonic::async_trait]
impl McpBridge for BridgeService {
    type StreamStream = Pin<Box<dyn Stream<Item = Result<Envelope, Status>> + Send + 'static>>;

    async fn stream(
        &self,
        request: Request<tonic::Streaming<Envelope>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        authorize(request.metadata(), self.state.bearer_token.as_deref())?;

        let client_id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel::<Result<Envelope, Status>>(GRPC_CLIENT_BUFFER);
        {
            let mut clients = self.state.clients.lock().await;
            clients.insert(client_id.clone(), tx);
        }

        let child = self.state.child.clone();
        let clients = self.state.clients.clone();
        let mut incoming = request.into_inner();
        tokio::spawn(async move {
            loop {
                match incoming.message().await {
                    Ok(Some(envelope)) => {
                        if envelope.json_rpc.trim().is_empty() {
                            continue;
                        }

                        let Ok(mut json) =
                            serde_json::from_str::<serde_json::Value>(&envelope.json_rpc)
                        else {
                            tracing::error!("Ignoring invalid JSON envelope from gRPC client");
                            continue;
                        };

                        if let Some(id) = json.get("id").cloned() {
                            let prefixed = prefix_id(&client_id, &id);
                            if let Some(obj) = json.as_object_mut() {
                                obj.insert("id".to_string(), prefixed);
                            }
                        }

                        if let Err(err) = child.send(&json).await {
                            tracing::error!("Failed to write gRPC message to stdio child: {err}");
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::error!("gRPC stream read error: {err}");
                        break;
                    }
                }
            }

            let mut clients = clients.lock().await;
            clients.remove(&client_id);
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn health(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        authorize(request.metadata(), self.state.bearer_token.as_deref())?;
        Ok(Response::new(HealthResponse {
            ok: self.state.child.is_alive().await,
            message: "ok".to_string(),
        }))
    }
}

pub async fn run(
    config: Config,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    let stdio_cmd = config.stdio.clone().ok_or("stdio command is required")?;

    tracing::info!("  - port: {}", config.port);
    tracing::info!("  - stdio: {}", stdio_cmd);

    let spec = parse_command_spec(&stdio_cmd)?;
    let child = Arc::new(StdioChild::new(spec, true));
    let initial_args = runtime.get_effective(None).await;
    child.spawn(&initial_args).await?;

    let state = AppState {
        clients: Arc::new(Mutex::new(HashMap::new())),
        child: child.clone(),
        seq: Arc::new(AtomicU64::new(0)),
        bearer_token: config.runtime_admin_token.clone(),
    };

    let runtime_child = child.clone();
    let runtime_store = runtime.clone();
    tokio::spawn(async move {
        while let Some(req) = updates.recv().await {
            let result = match req.update.scope {
                RuntimeScope::Global => {
                    let update_result = runtime_store.update_global(req.update.update).await;
                    if update_result.restart_needed {
                        let args = runtime_store.get_effective(None).await;
                        if runtime_child.restart(&args).await.is_err() {
                            RuntimeApplyResult::error("Failed to restart child")
                        } else {
                            RuntimeApplyResult::ok("Restarted child with new runtime args", true)
                        }
                    } else {
                        RuntimeApplyResult::ok("Updated runtime args", false)
                    }
                }
                RuntimeScope::Session(_) => RuntimeApplyResult::error(
                    "Per-session runtime overrides are not supported for stdio→gRPC",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let clients = state.clients.clone();
    let seq = state.seq.clone();
    let mut rx = child.subscribe();
    tokio::spawn(async move {
        loop {
            let msg = match rx.recv().await {
                Ok(msg) => msg,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        "stdio->grpc child output receiver lagged by {skipped} messages; continuing"
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            let mut target_id: Option<String> = None;
            let mut outgoing = msg.clone();
            if let Some((client_id, raw_id)) = strip_prefixed_id(&msg) {
                target_id = Some(client_id);
                if let Some(obj) = outgoing.as_object_mut() {
                    obj.insert("id".to_string(), raw_id);
                }
            }

            let envelope = Envelope {
                json_rpc: outgoing.to_string(),
                metadata: HashMap::new(),
                session_id: String::new(),
                seq: seq.fetch_add(1, Ordering::SeqCst) + 1,
            };

            if let Some(target) = target_id {
                let sender = {
                    let clients_guard = clients.lock().await;
                    clients_guard.get(&target).cloned()
                };
                if let Some(sender) = sender {
                    if sender.send(Ok(envelope)).await.is_err() {
                        let mut clients_guard = clients.lock().await;
                        clients_guard.remove(&target);
                    }
                }
                continue;
            }

            let senders: Vec<(String, GrpcEnvelopeSender)> = {
                let clients_guard = clients.lock().await;
                clients_guard
                    .iter()
                    .map(|(id, sender)| (id.clone(), sender.clone()))
                    .collect()
            };

            let mut dead = Vec::new();
            for (id, sender) in senders {
                if sender.send(Ok(envelope.clone())).await.is_err() {
                    dead.push(id);
                }
            }
            if !dead.is_empty() {
                let mut clients_guard = clients.lock().await;
                for id in dead {
                    clients_guard.remove(&id);
                }
            }
        }
    });

    install_signal_handlers(None);

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], config.port).into();
    tracing::info!("Listening on port {}", config.port);
    tracing::info!("gRPC endpoint: grpc://localhost:{}", config.port);

    Server::builder()
        .add_service(McpBridgeServer::new(BridgeService { state }))
        .serve(addr)
        .await
        .map_err(|err| err.to_string())
}

fn authorize(metadata: &tonic::metadata::MetadataMap, token: Option<&str>) -> Result<(), Status> {
    let Some(token) = token else {
        return Ok(());
    };
    let Some(auth) = metadata
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    else {
        return Err(Status::unauthenticated("missing authorization metadata"));
    };

    if auth == format!("Bearer {token}") {
        Ok(())
    } else {
        Err(Status::unauthenticated("invalid bearer token"))
    }
}

fn prefix_id(client_id: &str, id: &serde_json::Value) -> serde_json::Value {
    match id {
        serde_json::Value::String(s) => serde_json::Value::String(format!("{client_id}:{s}")),
        serde_json::Value::Number(n) => serde_json::Value::String(format!("{client_id}:{n}")),
        _ => serde_json::Value::String(format!("{client_id}:{id}")),
    }
}

fn strip_prefixed_id(message: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    let id_val = message.get("id")?;
    let id_str = id_val.as_str()?;
    let mut parts = id_str.splitn(2, ':');
    let client = parts.next()?.to_string();
    let raw = parts.next()?.to_string();
    let raw_id = raw
        .parse::<i64>()
        .map(|num| serde_json::Value::Number(num.into()))
        .unwrap_or_else(|_| serde_json::Value::String(raw));
    Some((client, raw_id))
}
