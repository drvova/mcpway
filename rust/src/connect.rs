use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::mpsc;
use url::Url;

use crate::config::{Config, ConnectConfig, ConnectProtocol, CorsConfig, OutputTransport};
use crate::discovery::registry::{resolve_server, ResolvedImportedServer};
use crate::gateways::{grpc_to_stdio, sse_to_stdio, streamable_http_to_stdio, ws_to_stdio};
use crate::generator;
use crate::oauth;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::RuntimeUpdateRequest;
use crate::support::command_spec::parse_command_spec;
use crate::support::telemetry::init_telemetry;
use crate::transport::reliability::{
    run_with_retry, CircuitBreaker, CircuitBreakerPolicy, RetryPolicy,
};
use crate::types::RuntimeArgs;

type StdioLaunchSpec = (String, String, Vec<String>, HashMap<String, String>);

pub async fn run(config: ConnectConfig) -> Result<(), String> {
    if let Some(oauth_cfg) = config.oauth.as_ref() {
        if oauth_cfg.logout {
            let removed = oauth::logout(oauth_cfg)?;
            println!("[mcpway] OAuth logout removed {removed} cached token record(s)");
            return Ok(());
        }
    }

    if config.stdio_cmd.is_some() || config.stdio_wrapper.is_some() {
        return run_connect_stdio_mode(config).await;
    }

    if let Some(server_name) = config.server.clone() {
        return run_server_mode(config, &server_name).await;
    }

    let endpoint = config
        .endpoint
        .clone()
        .ok_or_else(|| {
            "connect requires one mode: <ENDPOINT>, --server <NAME>, --stdio-cmd <CMD>, or --stdio-wrapper <PATH>".to_string()
        })?;
    let protocol = if let Some(explicit) = config.protocol {
        explicit
    } else {
        infer_protocol(&endpoint)?
    };

    run_remote_mode(config, endpoint, protocol).await
}

async fn run_server_mode(config: ConnectConfig, server_name: &str) -> Result<(), String> {
    let resolved = resolve_server(server_name, config.registry_path.as_deref())?;

    match resolved {
        ResolvedImportedServer::Remote {
            name,
            endpoint,
            protocol,
            headers,
        } => {
            let mut merged_headers = headers;
            for (key, value) in &config.headers {
                merged_headers.insert(key.clone(), value.clone());
            }

            let effective_protocol = config.protocol.unwrap_or(protocol);
            let mut effective = config;
            effective.endpoint = Some(endpoint.clone());
            effective.headers = merged_headers;
            if effective.save_profile_dir.is_some() && effective.profile_name.is_none() {
                effective.profile_name = Some(name);
            }

            run_remote_mode(effective, endpoint, effective_protocol).await
        }
        ResolvedImportedServer::Stdio {
            name,
            command,
            args,
            env,
        } => {
            if config.protocol.is_some() {
                return Err(
                    "--protocol is only supported for remote endpoints or remote imported servers"
                        .to_string(),
                );
            }
            if config.save_profile_dir.is_some() {
                return Err(
                    "--save-profile is not supported for imported stdio servers; use `import --save-profiles`"
                        .to_string(),
                );
            }
            if let Some(dir) = config.save_wrapper_dir.as_deref() {
                save_stdio_wrapper(dir, &name, &command, &args, &env)?;
            }
            run_stdio_mode(config.log_level, &name, command, args, env).await
        }
    }
}

async fn run_remote_mode(
    mut config: ConnectConfig,
    endpoint: String,
    protocol: ConnectProtocol,
) -> Result<(), String> {
    if let Some(oauth_cfg) = config.oauth.as_ref() {
        let token = oauth::resolve_access_token(oauth_cfg).await?;
        let has_authorization = has_authorization_header(&config.headers);
        if oauth_cfg.login || !has_authorization {
            config
                .headers
                .insert("Authorization".to_string(), format!("Bearer {token}"));
        }
    }

    if config.save_profile_dir.is_some() {
        let mut profile_config = config.clone();
        profile_config.endpoint = Some(endpoint.clone());
        generator::save_connect_profile(&profile_config, protocol)?;
    }

    let _telemetry = init_telemetry(
        config.log_level,
        OutputTransport::Stdio,
        "connect",
        protocol.as_str(),
    );
    tracing::info!("Starting ad-hoc connect mode...");
    tracing::info!("  - endpoint: {endpoint}");
    tracing::info!("  - protocol: {}", protocol.as_str());
    tracing::info!("  - retry-attempts: {}", config.retry_attempts);
    tracing::info!(
        "  - retryBackoff: {}..{}ms",
        config.retry_base_delay_ms,
        config.retry_max_delay_ms
    );
    tracing::info!(
        "  - circuit: failures={} cooldown={}ms",
        config.circuit_failure_threshold,
        config.circuit_cooldown_ms
    );

    let retry_policy = RetryPolicy {
        max_retries: config.retry_attempts,
        base_delay: Duration::from_millis(config.retry_base_delay_ms),
        max_delay: Duration::from_millis(config.retry_max_delay_ms),
    };
    let mut circuit_breaker = CircuitBreaker::new(CircuitBreakerPolicy {
        failure_threshold: config.circuit_failure_threshold,
        cooldown: Duration::from_millis(config.circuit_cooldown_ms),
    });

    run_with_retry(
        "connect-remote",
        retry_policy,
        &mut circuit_breaker,
        || async {
            let runtime_store = RuntimeArgsStore::new(RuntimeArgs {
                headers: config.headers.clone(),
                ..Default::default()
            });
            let (_update_tx, update_rx) = mpsc::channel::<RuntimeUpdateRequest>(32);

            match protocol {
                ConnectProtocol::Sse => {
                    let gateway_config =
                        to_gateway_config(&config, &endpoint, ConnectProtocol::Sse);
                    sse_to_stdio::run(gateway_config, runtime_store, update_rx).await
                }
                ConnectProtocol::StreamableHttp => {
                    let gateway_config =
                        to_gateway_config(&config, &endpoint, ConnectProtocol::StreamableHttp);
                    streamable_http_to_stdio::run(gateway_config, runtime_store, update_rx).await
                }
                ConnectProtocol::Ws => {
                    ws_to_stdio::run(
                        endpoint.clone(),
                        config.protocol_version.clone(),
                        runtime_store,
                        update_rx,
                    )
                    .await
                }
                ConnectProtocol::Grpc => {
                    grpc_to_stdio::run(
                        endpoint.clone(),
                        config.protocol_version.clone(),
                        runtime_store,
                        update_rx,
                    )
                    .await
                }
            }
        },
    )
    .await
}

async fn run_stdio_mode(
    log_level: crate::config::LogLevel,
    name: &str,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> Result<(), String> {
    let _telemetry = init_telemetry(log_level, OutputTransport::Stdio, "connect", "stdio");
    tracing::info!("Starting imported stdio server mode...");
    tracing::info!("  - server: {name}");
    tracing::info!("  - command: {command}");

    let status = Command::new(&command)
        .args(&args)
        .envs(&env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|err| format!("Failed to spawn '{command}': {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Imported stdio server '{}' exited with status {}",
            name, status
        ))
    }
}

async fn run_connect_stdio_mode(config: ConnectConfig) -> Result<(), String> {
    let (name, command, args, env) = resolve_connect_stdio_command(&config)?;
    if let Some(dir) = config.save_wrapper_dir.as_deref() {
        save_stdio_wrapper(dir, &name, &command, &args, &env)?;
    }
    run_stdio_mode(config.log_level, &name, command, args, env).await
}

fn resolve_connect_stdio_command(config: &ConnectConfig) -> Result<StdioLaunchSpec, String> {
    if let Some(cmd) = config.stdio_cmd.as_deref() {
        let spec = parse_command_spec(cmd)?;
        let mut args = spec.args;
        args.extend(config.stdio_args.clone());
        return Ok((
            "ad-hoc-stdio".to_string(),
            spec.program,
            args,
            config.stdio_env.clone(),
        ));
    }

    let wrapper_path = config
        .stdio_wrapper
        .as_ref()
        .ok_or_else(|| "Missing stdio mode source".to_string())?;

    if wrapper_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
    {
        return resolve_from_wrapper_json(wrapper_path, config);
    }

    let name = wrapper_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("wrapper-stdio")
        .to_string();
    Ok((
        name,
        wrapper_path.to_string_lossy().to_string(),
        config.stdio_args.clone(),
        config.stdio_env.clone(),
    ))
}

fn resolve_from_wrapper_json(
    wrapper_path: &Path,
    config: &ConnectConfig,
) -> Result<StdioLaunchSpec, String> {
    let body = std::fs::read_to_string(wrapper_path)
        .map_err(|err| format!("Failed to read wrapper {}: {err}", wrapper_path.display()))?;
    let value = serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|err| format!("Invalid wrapper JSON at {}: {err}", wrapper_path.display()))?;

    let command = value
        .pointer("/normalized/command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "Wrapper {} missing command field (expected normalized.command)",
                wrapper_path.display()
            )
        })?
        .to_string();

    let mut args = value
        .pointer("/normalized/args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|val| val.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    args.extend(config.stdio_args.clone());

    let mut env = value
        .pointer("/normalized/env_template")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(key, raw)| {
                    let value = raw
                        .as_str()
                        .map(resolve_env_template_value)
                        .unwrap_or_default();
                    (key.clone(), value)
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    for (key, value) in &config.stdio_env {
        env.insert(key.clone(), value.clone());
    }

    let name = wrapper_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("wrapper-stdio")
        .to_string();
    Ok((name, command, args, env))
}

fn resolve_env_template_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with("${") && trimmed.ends_with('}') && trimmed.len() > 3 {
        let key = &trimmed[2..trimmed.len() - 1];
        if let Ok(found) = std::env::var(key) {
            return found;
        }
    }
    value.to_string()
}

fn save_stdio_wrapper(
    output_dir: &Path,
    name: &str,
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create {}: {err}", output_dir.display()))?;
    let file_name = sanitize_wrapper_name(name);
    let path = output_dir.join(format!("{file_name}-stdio-wrapper.json"));
    let payload = serde_json::json!({
        "schema_version": "1",
        "name": name,
        "normalized": {
            "command": command,
            "args": args,
            "env_template": env,
        },
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload)
            .map_err(|err| format!("Failed to serialize stdio wrapper payload: {err}"))?,
    )
    .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    println!("[mcpway] Saved stdio wrapper: {}", path.display());
    Ok(path)
}

fn sanitize_wrapper_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "wrapper".to_string()
    } else {
        trimmed.to_string()
    }
}

fn has_authorization_header(headers: &HashMap<String, String>) -> bool {
    headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"))
}

pub fn infer_protocol(endpoint: &str) -> Result<ConnectProtocol, String> {
    let url = Url::parse(endpoint).map_err(|err| format!("Invalid endpoint URL: {err}"))?;

    match url.scheme() {
        "ws" | "wss" => Ok(ConnectProtocol::Ws),
        "grpc" | "grpcs" => Ok(ConnectProtocol::Grpc),
        "http" | "https" => {
            let has_sse_segment = url
                .path_segments()
                .map(|mut segments| segments.any(|segment| segment.eq_ignore_ascii_case("sse")))
                .unwrap_or(false);

            if has_sse_segment {
                Ok(ConnectProtocol::Sse)
            } else {
                Ok(ConnectProtocol::StreamableHttp)
            }
        }
        other => Err(format!(
            "Unsupported endpoint scheme '{other}'. Use ws://, wss://, http://, https://, grpc://, or grpcs://"
        )),
    }
}

fn to_gateway_config(config: &ConnectConfig, endpoint: &str, protocol: ConnectProtocol) -> Config {
    let (sse, streamable_http) = match protocol {
        ConnectProtocol::Sse => (Some(endpoint.to_string()), None),
        ConnectProtocol::StreamableHttp => (None, Some(endpoint.to_string())),
        ConnectProtocol::Ws | ConnectProtocol::Grpc => (None, None),
    };

    Config {
        stdio: None,
        sse,
        streamable_http,
        output_transport: OutputTransport::Stdio,
        port: 8000,
        base_url: String::new(),
        sse_path: "/sse".to_string(),
        message_path: "/message".to_string(),
        streamable_http_path: "/mcp".to_string(),
        log_level: config.log_level,
        cors: CorsConfig::Disabled,
        health_endpoints: Vec::new(),
        headers: config.headers.clone(),
        env: HashMap::new(),
        stateful: false,
        session_timeout: None,
        protocol_version: config.protocol_version.clone(),
        runtime_prompt: false,
        runtime_admin_port: None,
        runtime_admin_host: "127.0.0.1".to_string(),
        runtime_admin_token: None,
        retry_attempts: config.retry_attempts,
        retry_base_delay_ms: config.retry_base_delay_ms,
        retry_max_delay_ms: config.retry_max_delay_ms,
        circuit_failure_threshold: config.circuit_failure_threshold,
        circuit_cooldown_ms: config.circuit_cooldown_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_protocol_uses_ws_scheme() {
        assert_eq!(
            infer_protocol("wss://example.com/ws").ok(),
            Some(ConnectProtocol::Ws)
        );
    }

    #[test]
    fn infer_protocol_detects_sse_path() {
        assert_eq!(
            infer_protocol("https://example.com/sse").ok(),
            Some(ConnectProtocol::Sse)
        );
    }

    #[test]
    fn infer_protocol_defaults_http_to_streamable() {
        assert_eq!(
            infer_protocol("https://example.com/mcp").ok(),
            Some(ConnectProtocol::StreamableHttp)
        );
    }

    #[test]
    fn infer_protocol_ignores_transport_query_hint() {
        assert_eq!(
            infer_protocol("https://example.com/mcp?transport=sse").ok(),
            Some(ConnectProtocol::StreamableHttp)
        );
    }

    #[test]
    fn infer_protocol_uses_grpc_scheme() {
        assert_eq!(
            infer_protocol("grpc://example.com/mcp").ok(),
            Some(ConnectProtocol::Grpc)
        );
        assert_eq!(
            infer_protocol("grpcs://example.com/mcp").ok(),
            Some(ConnectProtocol::Grpc)
        );
    }
}
