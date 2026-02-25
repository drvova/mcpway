use std::fs::File;
use std::io::{BufRead, BufReader};

use serde::Deserialize;
use tokio::sync::mpsc;

use crate::runtime::store::RuntimeArgsUpdate;
use crate::runtime::{RuntimeScope, RuntimeUpdate};

#[derive(Debug, Deserialize)]
struct PromptInput {
    scope: String,
    session_id: Option<String>,
    extra_cli_args: Option<Vec<String>>,
    env: Option<std::collections::HashMap<String, String>>,
    headers: Option<std::collections::HashMap<String, String>>,
}

pub fn spawn_prompt() -> mpsc::Receiver<RuntimeUpdate> {
    let (tx, rx) = mpsc::channel(32);

    std::thread::spawn(move || {
        let Ok(file) = File::open("/dev/tty") else {
            tracing::error!("Runtime prompt disabled: /dev/tty unavailable");
            return;
        };
        let reader = BufReader::new(file);
        tracing::info!("Runtime prompt enabled. Enter JSON per line.");
        tracing::info!(
            "{}",
            "Example: {\"scope\":\"global\",\"extra_cli_args\":[\"--token\",\"abc\"],\"env\":{\"API_KEY\":\"xyz\"},\"headers\":{\"Authorization\":\"Bearer 123\"}}"
        );
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<PromptInput>(trimmed) {
                        Ok(input) => {
                            let scope = match input.scope.as_str() {
                                "global" => RuntimeScope::Global,
                                "session" => {
                                    if let Some(id) = input.session_id.clone() {
                                        RuntimeScope::Session(id)
                                    } else {
                                        tracing::error!(
                                            "Prompt input missing session_id for session scope"
                                        );
                                        continue;
                                    }
                                }
                                other => {
                                    tracing::error!("Unknown scope: {other}");
                                    continue;
                                }
                            };
                            let update = RuntimeArgsUpdate {
                                extra_cli_args: input.extra_cli_args,
                                env: input.env,
                                headers: input.headers,
                            };
                            let update_msg = RuntimeUpdate { scope, update };
                            if tx.blocking_send(update_msg).is_err() {
                                tracing::error!("Runtime prompt channel closed");
                                break;
                            }
                        }
                        Err(err) => {
                            tracing::error!("Invalid JSON input: {err}");
                        }
                    }
                }
                Err(err) => {
                    tracing::error!("Runtime prompt read error: {err}");
                    break;
                }
            }
        }
    });

    rx
}
