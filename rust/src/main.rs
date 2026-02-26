mod config;
mod connect;
mod discovery;
mod gateways;
mod generator;
mod grpc_proto;
mod logs;
mod oauth;
mod runtime;
mod support;
mod transport;
mod types;
mod web;

use std::collections::{BTreeMap, HashMap};
use std::net::ToSocketAddrs;
use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};

use crate::config::{
    parse_cli_command, CliCommand, Config, ConnectConfig, ConnectProtocol, DiscoverConfig,
    DiscoverScope, DiscoverSortBy, DiscoverTransport, GenerateConfig, ImportConfig, ImportSource,
    LogLevel, OutputTransport, SortOrder,
};
use crate::discovery::{
    DiscoverOptions, DiscoveredServer, DiscoveredTransport, DiscoveryConflict,
    DiscoverySearchOptions, DiscoverySortField, DiscoverySortOrder, SourceKind,
};
use crate::gateways::{
    sse_to_stdio, stdio_to_grpc, stdio_to_sse, stdio_to_stdio, stdio_to_streamable_http,
    stdio_to_ws, streamable_http_to_stdio,
};
use crate::runtime::admin::{spawn_admin_server, AdminServerOptions};
use crate::runtime::prompt::spawn_prompt;
use crate::runtime::store::RuntimeArgsStore;
use crate::runtime::{RuntimeApplyResult, RuntimeUpdate, RuntimeUpdateRequest};
use crate::support::telemetry::init_telemetry;
use crate::types::RuntimeArgs;

#[tokio::main]
async fn main() {
    let cli_command = match parse_cli_command() {
        Ok(command) => command,
        Err(err) => {
            eprintln!("[mcpway] Error: {err}");
            std::process::exit(1);
        }
    };

    match cli_command {
        CliCommand::Run(config) => {
            if let Err(err) = run_gateway(*config).await {
                tracing::error!("Fatal error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Generate(config) => {
            if let Err(err) = generator::run_generate(&config) {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Regenerate(config) => {
            if let Err(err) = generator::run_regenerate(&config) {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Connect(config) => {
            if let Err(err) = connect::run(*config).await {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Discover(config) => {
            if let Err(err) = run_discover(config) {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Import(config) => {
            if let Err(err) = run_import(config) {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Logs(config) => {
            if let Err(err) = logs::run(config).await {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
        CliCommand::Web(config) => {
            if let Err(err) = web::run(config).await {
                eprintln!("[mcpway] Error: {err}");
                std::process::exit(1);
            }
        }
    }
}

async fn run_gateway(config: Config) -> Result<(), String> {
    let _telemetry = init_telemetry(
        config.log_level,
        config.output_transport,
        "gateway",
        output_transport_label(config.output_transport),
    );
    tracing::info!("Starting...");
    tracing::info!("mcpway gateway runtime initialized",);
    tracing::info!("  - output-transport: {:?}", config.output_transport);

    let runtime_store = RuntimeArgsStore::new(RuntimeArgs {
        headers: config.headers.clone(),
        env: config.env.clone(),
        ..Default::default()
    });

    let (update_tx, update_rx) = mpsc::channel::<RuntimeUpdateRequest>(32);

    if config.runtime_prompt {
        let mut prompt_rx = spawn_prompt();
        let update_tx = update_tx.clone();
        tokio::spawn(async move {
            while let Some(update) = prompt_rx.recv().await {
                let (resp_tx, resp_rx) = oneshot::channel();
                if update_tx
                    .send(RuntimeUpdateRequest {
                        update,
                        respond_to: resp_tx,
                    })
                    .await
                    .is_err()
                {
                    tracing::error!("Runtime update channel closed");
                    break;
                }
                if let Ok(result) = resp_rx.await {
                    tracing::info!("Runtime update: {}", result.message);
                }
            }
        });
    }

    if let Some(port) = config.runtime_admin_port {
        let addr = resolve_bind_addr(&config.runtime_admin_host, port)?;
        let update_tx = update_tx.clone();
        let handler: Arc<
            dyn Fn(RuntimeUpdate) -> BoxFuture<'static, RuntimeApplyResult> + Send + Sync,
        > = Arc::new(move |update: RuntimeUpdate| {
            let update_tx = update_tx.clone();
            Box::pin(async move {
                let (resp_tx, resp_rx) = oneshot::channel();
                if update_tx
                    .send(RuntimeUpdateRequest {
                        update,
                        respond_to: resp_tx,
                    })
                    .await
                    .is_err()
                {
                    return RuntimeApplyResult::error("Runtime update channel closed");
                }
                resp_rx
                    .await
                    .unwrap_or_else(|_| RuntimeApplyResult::error("Runtime update handler failed"))
            }) as BoxFuture<'static, RuntimeApplyResult>
        });
        let runtime_clone = runtime_store.clone();
        let admin_options = AdminServerOptions {
            bearer_token: config.runtime_admin_token.clone(),
            loopback_only: addr.ip().is_loopback(),
            discovery_project_root: discovery::resolve_project_root(None).ok(),
            discovery_source: None,
        };
        tokio::spawn(async move {
            spawn_admin_server(addr, runtime_clone, handler, admin_options).await;
        });
    }

    let result = if config.stdio.is_some() {
        match config.output_transport {
            OutputTransport::Sse => stdio_to_sse::run(config, runtime_store, update_rx).await,
            OutputTransport::Ws => stdio_to_ws::run(config, runtime_store, update_rx).await,
            OutputTransport::StreamableHttp => {
                stdio_to_streamable_http::run(config, runtime_store, update_rx).await
            }
            OutputTransport::Grpc => stdio_to_grpc::run(config, runtime_store, update_rx).await,
            OutputTransport::Stdio => stdio_to_stdio::run(config, runtime_store, update_rx).await,
        }
    } else if config.sse.is_some() {
        match config.output_transport {
            OutputTransport::Stdio => sse_to_stdio::run(config, runtime_store, update_rx).await,
            OutputTransport::Sse
            | OutputTransport::Ws
            | OutputTransport::StreamableHttp
            | OutputTransport::Grpc => Err("sse→output transport not supported".to_string()),
        }
    } else if config.streamable_http.is_some() {
        match config.output_transport {
            OutputTransport::Stdio => {
                streamable_http_to_stdio::run(config, runtime_store, update_rx).await
            }
            OutputTransport::Sse
            | OutputTransport::Ws
            | OutputTransport::StreamableHttp
            | OutputTransport::Grpc => {
                Err("streamable-http→output transport not supported".to_string())
            }
        }
    } else {
        Err("Invalid input transport".to_string())
    };

    result
}

fn run_discover(config: DiscoverConfig) -> Result<(), String> {
    let options = DiscoverOptions {
        from: import_source_to_kind(config.from),
        project_root: config.project_root.clone(),
    };
    let full_report = discovery::discover(&options)?;

    if config.strict_conflicts && !full_report.conflicts.is_empty() {
        return Err(render_conflicts_error(&full_report.conflicts));
    }

    let search = discover_search_options_from_config(&config);
    let report = discovery::apply_search(&full_report, &search);

    if config.print_json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|err| format!("Failed to serialize discover report: {err}"))?;
        println!("{json}");
    } else {
        print_discover_human(&report, full_report.servers.len());
    }

    Ok(())
}

fn run_import(config: ImportConfig) -> Result<(), String> {
    let options = DiscoverOptions {
        from: import_source_to_kind(config.from),
        project_root: config.project_root.clone(),
    };
    let report = discovery::discover(&options)?;

    if config.strict_conflicts && !report.conflicts.is_empty() {
        return Err(render_conflicts_error(&report.conflicts));
    }

    let registry_path = config
        .registry_path
        .clone()
        .unwrap_or_else(discovery::registry::default_registry_path);
    discovery::registry::write_registry(&registry_path, &report.servers)?;

    let mut generated_profiles = 0usize;
    if let Some(save_dir) = config.save_profiles_dir.as_deref() {
        generated_profiles = save_import_profiles(
            save_dir,
            &report.servers,
            config.bundle_mcpway,
            config.compile_wrapper,
        )?;
    }

    if config.print_json {
        let payload = serde_json::json!({
            "registry_path": registry_path.to_string_lossy().to_string(),
            "project_root": report.project_root,
            "imported": report.servers.len(),
            "generated_profiles": generated_profiles,
            "conflicts": report.conflicts,
            "issues": report.issues,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload)
                .map_err(|err| format!("Failed to serialize import output: {err}"))?
        );
    } else {
        println!(
            "[mcpway] Imported {} server(s) into {}",
            report.servers.len(),
            registry_path.display()
        );
        if generated_profiles > 0 {
            println!("[mcpway] Generated {generated_profiles} profile artifact(s)");
        }
        if !report.conflicts.is_empty() {
            println!(
                "[mcpway] Conflicts resolved by source priority: {}",
                report.conflicts.len()
            );
        }
        if !report.issues.is_empty() {
            println!(
                "[mcpway] Warnings emitted during import: {}",
                report.issues.len()
            );
        }
    }

    Ok(())
}

fn import_source_to_kind(from: ImportSource) -> Option<SourceKind> {
    match from {
        ImportSource::Auto => None,
        ImportSource::Cursor => Some(SourceKind::Cursor),
        ImportSource::Claude => Some(SourceKind::Claude),
        ImportSource::Codex => Some(SourceKind::Codex),
        ImportSource::Windsurf => Some(SourceKind::Windsurf),
        ImportSource::Opencode => Some(SourceKind::OpenCode),
        ImportSource::Nodecode => Some(SourceKind::Nodecode),
        ImportSource::Vscode => Some(SourceKind::VsCode),
    }
}

fn discover_search_options_from_config(config: &DiscoverConfig) -> DiscoverySearchOptions {
    DiscoverySearchOptions {
        query: config.search.clone(),
        transport: config.transport.map(discover_transport_to_discovered),
        scope: config.scope.map(discover_scope_to_discovery_scope),
        source: None,
        enabled_only: config.enabled_only,
        sort_by: discover_sort_to_search_sort(config.sort_by),
        sort_order: sort_order_to_discovery_sort_order(config.order),
        offset: config.offset,
        limit: config.limit,
    }
}

fn discover_transport_to_discovered(transport: DiscoverTransport) -> DiscoveredTransport {
    match transport {
        DiscoverTransport::Stdio => DiscoveredTransport::Stdio,
        DiscoverTransport::Sse => DiscoveredTransport::Sse,
        DiscoverTransport::Ws => DiscoveredTransport::Ws,
        DiscoverTransport::StreamableHttp => DiscoveredTransport::StreamableHttp,
        DiscoverTransport::Grpc => DiscoveredTransport::Grpc,
    }
}

fn discover_scope_to_discovery_scope(scope: DiscoverScope) -> discovery::DiscoveryScope {
    match scope {
        DiscoverScope::Project => discovery::DiscoveryScope::Project,
        DiscoverScope::Global => discovery::DiscoveryScope::Global,
    }
}

fn discover_sort_to_search_sort(sort: DiscoverSortBy) -> DiscoverySortField {
    match sort {
        DiscoverSortBy::Name => DiscoverySortField::Name,
        DiscoverSortBy::Source => DiscoverySortField::Source,
        DiscoverSortBy::Scope => DiscoverySortField::Scope,
        DiscoverSortBy::Transport => DiscoverySortField::Transport,
        DiscoverSortBy::OriginPath => DiscoverySortField::OriginPath,
    }
}

fn sort_order_to_discovery_sort_order(order: SortOrder) -> DiscoverySortOrder {
    match order {
        SortOrder::Asc => DiscoverySortOrder::Asc,
        SortOrder::Desc => DiscoverySortOrder::Desc,
    }
}

fn render_conflicts_error(conflicts: &[DiscoveryConflict]) -> String {
    let mut out = String::from("Discovery conflicts detected with --strict-conflicts:\n");
    for conflict in conflicts {
        out.push_str(&format!(
            "  - '{}': kept {} ({:?}), dropped {} ({:?})\n",
            conflict.name,
            conflict.kept_source.as_str(),
            conflict.kept_scope,
            conflict.dropped_source.as_str(),
            conflict.dropped_scope
        ));
    }
    out
}

fn print_discover_human(report: &discovery::DiscoveryReport, total_servers: usize) {
    println!(
        "[mcpway] Discovered {} server(s) in {}",
        report.servers.len(),
        report.project_root
    );
    if report.servers.len() != total_servers {
        println!(
            "[mcpway] Filtered results: {} / {} server(s)",
            report.servers.len(),
            total_servers
        );
    }
    for server in &report.servers {
        println!(
            "  - {} [{} {} {}]",
            server.name,
            transport_label(server.transport),
            server.source.as_str(),
            scope_label(server.scope)
        );
    }

    if !report.conflicts.is_empty() {
        println!(
            "[mcpway] Resolved {} cross-source conflict(s)",
            report.conflicts.len()
        );
    }
    if !report.issues.is_empty() {
        println!("[mcpway] Warnings:");
        for issue in &report.issues {
            println!(
                "  - {} {}: {}",
                issue.source.as_str(),
                issue.origin_path,
                issue.message
            );
        }
    }
}

fn transport_label(transport: DiscoveredTransport) -> &'static str {
    match transport {
        DiscoveredTransport::Stdio => "stdio",
        DiscoveredTransport::Sse => "sse",
        DiscoveredTransport::Ws => "ws",
        DiscoveredTransport::StreamableHttp => "streamable-http",
        DiscoveredTransport::Grpc => "grpc",
    }
}

fn scope_label(scope: discovery::DiscoveryScope) -> &'static str {
    match scope {
        discovery::DiscoveryScope::Project => "project",
        discovery::DiscoveryScope::Global => "global",
    }
}

fn output_transport_label(output: OutputTransport) -> &'static str {
    match output {
        OutputTransport::Stdio => "stdio",
        OutputTransport::Sse => "sse",
        OutputTransport::Ws => "ws",
        OutputTransport::StreamableHttp => "streamable-http",
        OutputTransport::Grpc => "grpc",
    }
}

fn resolve_bind_addr(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
    let target = format!("{host}:{port}");
    target
        .to_socket_addrs()
        .map_err(|err| format!("Failed to resolve runtime admin host '{host}': {err}"))?
        .next()
        .ok_or_else(|| format!("No socket addresses resolved for runtime admin host '{host}'"))
}

fn save_import_profiles(
    base_dir: &Path,
    servers: &[DiscoveredServer],
    bundle_mcpway: bool,
    compile_wrapper: bool,
) -> Result<usize, String> {
    std::fs::create_dir_all(base_dir)
        .map_err(|err| format!("Failed to create {}: {err}", base_dir.display()))?;

    let mut count = 0usize;
    for server in servers {
        let profile_name = sanitize_name(&server.name);
        let output_dir = base_dir.join(&profile_name);
        match server.transport {
            DiscoveredTransport::Stdio => {
                save_stdio_profile(
                    server,
                    &output_dir,
                    &profile_name,
                    bundle_mcpway,
                    compile_wrapper,
                )?;
            }
            DiscoveredTransport::Sse
            | DiscoveredTransport::Ws
            | DiscoveredTransport::StreamableHttp
            | DiscoveredTransport::Grpc => {
                save_remote_profile(server, &output_dir, &profile_name)?;
            }
        }
        count += 1;
    }

    Ok(count)
}

fn save_stdio_profile(
    server: &DiscoveredServer,
    output_dir: &Path,
    artifact_name: &str,
    bundle_mcpway: bool,
    compile_wrapper: bool,
) -> Result<(), String> {
    let command = server
        .command
        .as_deref()
        .ok_or_else(|| format!("Server '{}' missing command", server.name))?;

    std::fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create {}: {err}", output_dir.display()))?;

    let definition_path = output_dir.join("imported-definition.json");
    let sanitized_env = server
        .env
        .keys()
        .map(|key| (key.clone(), format!("${{{key}}}")))
        .collect::<BTreeMap<_, _>>();
    let sanitized_headers = server
        .headers
        .keys()
        .map(|key| (key.clone(), "<redacted>".to_string()))
        .collect::<BTreeMap<_, _>>();
    let definition_json = serde_json::json!({
        "mcpServers": {
            server.name.clone(): {
                "command": command,
                "args": server.args.clone(),
                "env": sanitized_env,
                "headers": sanitized_headers,
            }
        }
    });
    std::fs::write(
        &definition_path,
        serde_json::to_string_pretty(&definition_json)
            .map_err(|err| format!("Failed to serialize imported definition: {err}"))?,
    )
    .map_err(|err| format!("Failed to write {}: {err}", definition_path.display()))?;

    let generate = GenerateConfig {
        definition: definition_path,
        server: Some(server.name.clone()),
        out: output_dir.to_path_buf(),
        artifact_name: Some(artifact_name.to_string()),
        bundle_mcpway,
        mcpway_binary: None,
        compile_wrapper,
    };
    generator::run_generate(&generate)
}

fn save_remote_profile(
    server: &DiscoveredServer,
    output_dir: &Path,
    profile_name: &str,
) -> Result<(), String> {
    let endpoint = server
        .url
        .clone()
        .ok_or_else(|| format!("Server '{}' missing remote URL", server.name))?;

    let protocol = match server.transport {
        DiscoveredTransport::Sse => ConnectProtocol::Sse,
        DiscoveredTransport::Ws => ConnectProtocol::Ws,
        DiscoveredTransport::StreamableHttp => ConnectProtocol::StreamableHttp,
        DiscoveredTransport::Grpc => ConnectProtocol::Grpc,
        DiscoveredTransport::Stdio => {
            return Err(format!(
                "Server '{}' is stdio, expected remote transport",
                server.name
            ));
        }
    };

    let connect = ConnectConfig {
        endpoint: Some(endpoint),
        server: None,
        stdio_cmd: None,
        stdio_args: Vec::new(),
        stdio_env: HashMap::new(),
        stdio_wrapper: None,
        save_wrapper_dir: None,
        protocol: Some(protocol),
        headers: server
            .headers
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>(),
        registry_path: None,
        save_profile_dir: Some(output_dir.to_path_buf()),
        profile_name: Some(profile_name.to_string()),
        log_level: LogLevel::Info,
        protocol_version: "2024-11-05".to_string(),
        oauth: None,
        retry_attempts: 2,
        retry_base_delay_ms: 250,
        retry_max_delay_ms: 2_000,
        circuit_failure_threshold: 3,
        circuit_cooldown_ms: 5_000,
    };

    generator::save_connect_profile(&connect, protocol)
}

fn sanitize_name(raw: &str) -> String {
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
        "server".to_string()
    } else {
        trimmed.to_string()
    }
}
