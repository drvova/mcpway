use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::{broadcast, mpsc};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::config::Config;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeScope, RuntimeUpdateRequest};
use crate::support::command_spec::parse_command_spec;
use crate::support::signals::install_signal_handlers;
use crate::support::stdio_child::StdioChild;

pub async fn run(
    config: Config,
    runtime: RuntimeArgsStore,
    mut updates: mpsc::Receiver<RuntimeUpdateRequest>,
) -> Result<(), String> {
    let stdio_cmd = config.stdio.clone().ok_or("stdio command is required")?;

    tracing::info!("  - stdio: {}", stdio_cmd);
    tracing::info!("  - output-transport: stdio");

    install_signal_handlers(None);

    let spec = parse_command_spec(&stdio_cmd)?;
    let child = Arc::new(StdioChild::new(spec, true));
    let initial_args = runtime.get_effective(None).await;
    child.spawn(&initial_args).await?;

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
                    } else if update_result.headers_changed {
                        RuntimeApplyResult::ok("Updated runtime headers", false)
                    } else {
                        RuntimeApplyResult::ok("No runtime changes applied", false)
                    }
                }
                RuntimeScope::Session(_) => RuntimeApplyResult::error(
                    "Per-session runtime overrides are not supported for stdio→stdio",
                ),
            };
            let _ = req.respond_to.send(result);
        }
    });

    let outbound_child = child.clone();
    tokio::spawn(async move {
        let mut rx = outbound_child.subscribe();
        loop {
            match rx.recv().await {
                Ok(message) => println!("{}", message),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        "stdio->stdio child output receiver lagged by {skipped} messages; continuing"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut lines = FramedRead::new(tokio::io::stdin(), LinesCodec::new());
    while let Some(line) = lines.next().await {
        let line = line.map_err(|err| err.to_string())?;
        if line.trim().is_empty() {
            continue;
        }

        let Ok(message_json) = serde_json::from_str::<serde_json::Value>(&line) else {
            tracing::error!("Invalid JSON from stdin: {line}");
            continue;
        };

        child.send(&message_json).await?;
    }

    child.shutdown().await;
    Ok(())
}
