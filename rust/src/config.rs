use clap::{Arg, ArgAction, ArgMatches, Command, ValueEnum};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::IsTerminal;
use std::path::PathBuf;

use crate::types::HeadersMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputTransport {
    Stdio,
    Sse,
    Ws,
    StreamableHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Debug,
    Info,
    None,
}

#[derive(Debug, Clone)]
pub enum CorsConfig {
    Disabled,
    AllowAll,
    AllowList { raw: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub stdio: Option<String>,
    pub sse: Option<String>,
    pub streamable_http: Option<String>,
    pub output_transport: OutputTransport,
    pub port: u16,
    pub base_url: String,
    pub sse_path: String,
    pub message_path: String,
    pub streamable_http_path: String,
    pub log_level: LogLevel,
    pub cors: CorsConfig,
    pub health_endpoints: Vec<String>,
    pub headers: HeadersMap,
    pub env: HashMap<String, String>,
    pub stateful: bool,
    pub session_timeout: Option<u64>,
    pub protocol_version: String,
    pub runtime_prompt: bool,
    pub runtime_admin_port: Option<u16>,
    pub runtime_admin_host: String,
    pub runtime_admin_token: Option<String>,
    pub retry_attempts: u32,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub circuit_failure_threshold: u32,
    pub circuit_cooldown_ms: u64,
}

#[derive(Debug, Clone)]
pub struct GenerateConfig {
    pub definition: PathBuf,
    pub server: Option<String>,
    pub out: PathBuf,
    pub artifact_name: Option<String>,
    pub bundle_mcpway: bool,
    pub mcpway_binary: Option<PathBuf>,
    pub compile_wrapper: bool,
}

#[derive(Debug, Clone)]
pub struct RegenerateConfig {
    pub metadata: PathBuf,
    pub definition: Option<PathBuf>,
    pub server: Option<String>,
    pub out: Option<PathBuf>,
    pub bundle_mcpway: Option<bool>,
    pub mcpway_binary: Option<PathBuf>,
    pub compile_wrapper: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConnectProtocol {
    Sse,
    Ws,
    StreamableHttp,
}

impl ConnectProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sse => "sse",
            Self::Ws => "ws",
            Self::StreamableHttp => "streamable-http",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OAuthFlow {
    Device,
    AuthCode,
}

#[derive(Debug, Clone)]
pub struct ConnectOauthConfig {
    pub profile: Option<String>,
    pub issuer: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub flow: OAuthFlow,
    pub no_browser: bool,
    pub cache_path: Option<PathBuf>,
    pub login: bool,
    pub logout: bool,
    pub audience: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConnectConfig {
    pub endpoint: Option<String>,
    pub server: Option<String>,
    pub stdio_cmd: Option<String>,
    pub stdio_args: Vec<String>,
    pub stdio_env: HashMap<String, String>,
    pub stdio_wrapper: Option<PathBuf>,
    pub save_wrapper_dir: Option<PathBuf>,
    pub protocol: Option<ConnectProtocol>,
    pub headers: HeadersMap,
    pub registry_path: Option<PathBuf>,
    pub save_profile_dir: Option<PathBuf>,
    pub profile_name: Option<String>,
    pub log_level: LogLevel,
    pub protocol_version: String,
    pub oauth: Option<ConnectOauthConfig>,
    pub retry_attempts: u32,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub circuit_failure_threshold: u32,
    pub circuit_cooldown_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiscoverTransport {
    Stdio,
    Sse,
    Ws,
    StreamableHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiscoverScope {
    Project,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiscoverSortBy {
    Name,
    Source,
    Scope,
    Transport,
    OriginPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImportSource {
    Auto,
    Cursor,
    Claude,
    Codex,
    Windsurf,
    Opencode,
    Nodecode,
    Vscode,
}

#[derive(Debug, Clone)]
pub struct DiscoverConfig {
    pub from: ImportSource,
    pub project_root: Option<PathBuf>,
    pub print_json: bool,
    pub strict_conflicts: bool,
    pub search: Option<String>,
    pub transport: Option<DiscoverTransport>,
    pub scope: Option<DiscoverScope>,
    pub enabled_only: bool,
    pub sort_by: DiscoverSortBy,
    pub order: SortOrder,
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ImportConfig {
    pub from: ImportSource,
    pub project_root: Option<PathBuf>,
    pub print_json: bool,
    pub strict_conflicts: bool,
    pub registry_path: Option<PathBuf>,
    pub save_profiles_dir: Option<PathBuf>,
    pub bundle_mcpway: bool,
    pub compile_wrapper: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogsLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogsTransport {
    Stdio,
    Sse,
    Ws,
    StreamableHttp,
    Connect,
}

impl LogsTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Ws => "ws",
            Self::StreamableHttp => "streamable-http",
            Self::Connect => "connect",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogsTailConfig {
    pub file: Option<PathBuf>,
    pub follow: bool,
    pub lines: usize,
    pub level: Option<LogsLevel>,
    pub transport: Option<LogsTransport>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub enum LogsConfig {
    Tail(LogsTailConfig),
}

#[derive(Debug, Clone)]
pub enum CliCommand {
    Run(Box<Config>),
    Generate(GenerateConfig),
    Regenerate(RegenerateConfig),
    Connect(Box<ConnectConfig>),
    Discover(DiscoverConfig),
    Import(ImportConfig),
    Logs(LogsConfig),
}

#[derive(Debug)]
pub enum ConfigError {
    MissingTransport,
    MultipleTransports,
    InvalidSessionTimeout(String),
    InvalidRuntimePort(String),
    InvalidArg(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::MissingTransport => {
                write!(
                    f,
                    "You must specify one of --stdio, --sse, or --streamable-http"
                )
            }
            ConfigError::MultipleTransports => {
                write!(
                    f,
                    "Specify only one of --stdio, --sse, or --streamable-http"
                )
            }
            ConfigError::InvalidSessionTimeout(msg) => write!(f, "{msg}"),
            ConfigError::InvalidRuntimePort(msg) => write!(f, "{msg}"),
            ConfigError::InvalidArg(msg) => write!(f, "{msg}"),
        }
    }
}

pub fn parse_cli_command() -> Result<CliCommand, ConfigError> {
    let raw_args: Vec<String> = env::args().collect();
    parse_cli_command_from(raw_args)
}

fn parse_cli_command_from(raw_args: Vec<String>) -> Result<CliCommand, ConfigError> {
    match raw_args.get(1).map(String::as_str) {
        Some("generate") => parse_generate_config_from(raw_args).map(CliCommand::Generate),
        Some("regenerate") => parse_regenerate_config_from(raw_args).map(CliCommand::Regenerate),
        Some("connect") => {
            parse_connect_config_from(raw_args).map(|cfg| CliCommand::Connect(Box::new(cfg)))
        }
        Some("discover") => parse_discover_config_from(raw_args).map(CliCommand::Discover),
        Some("import") => parse_import_config_from(raw_args).map(CliCommand::Import),
        Some("logs") => parse_logs_config_from(raw_args).map(CliCommand::Logs),
        _ => {
            if raw_args.len() <= 1 {
                eprintln!("{}", no_args_banner_text());
            }
            parse_config_from(raw_args).map(|cfg| CliCommand::Run(Box::new(cfg)))
        }
    }
}

fn parse_config_from(raw_args: Vec<String>) -> Result<Config, ConfigError> {
    let default_output = default_output_transport(&raw_args);
    let cors_input = parse_cors_flags(&raw_args);

    let matches = build_cli().get_matches_from(raw_args);

    let stdio = matches.get_one::<String>("stdio").cloned();
    let sse = matches.get_one::<String>("sse").cloned();
    let streamable_http = matches.get_one::<String>("streamable-http").cloned();

    let active = [stdio.is_some(), sse.is_some(), streamable_http.is_some()]
        .iter()
        .filter(|v| **v)
        .count();
    if active == 0 {
        return Err(ConfigError::MissingTransport);
    }
    if active > 1 {
        return Err(ConfigError::MultipleTransports);
    }

    let output_transport = matches
        .get_one::<OutputTransport>("output-transport")
        .copied()
        .or(default_output)
        .ok_or_else(|| {
            ConfigError::InvalidArg(
                "output-transport must be specified or inferable from input transport".into(),
            )
        })?;

    let port = matches
        .get_one::<String>("port")
        .cloned()
        .or_else(|| env::var("PORT").ok())
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8000);
    let base_url = matches
        .get_one::<String>("base-url")
        .cloned()
        .unwrap_or_default();
    let sse_path = matches
        .get_one::<String>("sse-path")
        .cloned()
        .unwrap_or_else(|| "/sse".to_string());
    let message_path = matches
        .get_one::<String>("message-path")
        .cloned()
        .unwrap_or_else(|| "/message".to_string());
    let streamable_http_path = matches
        .get_one::<String>("streamable-http-path")
        .cloned()
        .unwrap_or_else(|| "/mcp".to_string());
    let log_level = matches
        .get_one::<LogLevel>("log-level")
        .copied()
        .unwrap_or(LogLevel::Info);

    let health_endpoints: Vec<String> = matches
        .get_many::<String>("health-endpoint")
        .map(|vals| {
            vals.filter(|v| !v.is_empty())
                .map(|v| v.to_string())
                .collect()
        })
        .unwrap_or_default();

    let header_values: Vec<String> = matches
        .get_many::<String>("header")
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();

    let env_values: Vec<String> = matches
        .get_many::<String>("env")
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();

    let oauth2_bearer = matches.get_one::<String>("oauth2-bearer").cloned();
    let headers = parse_headers(&header_values, oauth2_bearer.as_deref())?;
    let env = parse_env_values(&env_values);

    let cors = if cors_input.present {
        if cors_input.allow_all {
            CorsConfig::AllowAll
        } else if !cors_input.values.is_empty() {
            CorsConfig::AllowList {
                raw: cors_input.values,
            }
        } else {
            CorsConfig::AllowAll
        }
    } else {
        CorsConfig::Disabled
    };

    let stateful = matches.get_flag("stateful");
    let session_timeout = if let Some(raw) = matches.get_one::<String>("session-timeout") {
        let val: i64 = raw.parse().map_err(|_| {
            ConfigError::InvalidSessionTimeout(format!(
                "session-timeout must be a positive number, received: {raw}"
            ))
        })?;
        if val <= 0 {
            return Err(ConfigError::InvalidSessionTimeout(format!(
                "session-timeout must be a positive number, received: {raw}"
            )));
        }
        Some(val as u64)
    } else {
        None
    };

    let protocol_version = matches
        .get_one::<String>("protocol-version")
        .cloned()
        .unwrap_or_else(|| "2024-11-05".to_string());

    let runtime_prompt = matches.get_flag("runtime-prompt");
    let runtime_admin_port = if let Some(raw) = matches.get_one::<String>("runtime-admin-port") {
        let val: i64 = raw.parse().map_err(|_| {
            ConfigError::InvalidRuntimePort(format!(
                "runtime-admin-port must be a valid port, received: {raw}"
            ))
        })?;
        if val <= 0 || val > u16::MAX as i64 {
            return Err(ConfigError::InvalidRuntimePort(format!(
                "runtime-admin-port must be in 1..=65535, received: {raw}"
            )));
        }
        Some(val as u16)
    } else {
        None
    };
    let runtime_admin_host = matches
        .get_one::<String>("runtime-admin-host")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let runtime_admin_token = matches
        .get_one::<String>("runtime-admin-token")
        .cloned()
        .or_else(|| env::var("MCPWAY_RUNTIME_ADMIN_TOKEN").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let retry_attempts = matches
        .get_one::<u32>("retry-attempts")
        .copied()
        .unwrap_or(2);
    let retry_base_delay_ms = matches
        .get_one::<u64>("retry-base-delay-ms")
        .copied()
        .unwrap_or(250);
    let retry_max_delay_ms = matches
        .get_one::<u64>("retry-max-delay-ms")
        .copied()
        .unwrap_or(2_000);
    let circuit_failure_threshold = matches
        .get_one::<u32>("circuit-failure-threshold")
        .copied()
        .unwrap_or(3);
    let circuit_cooldown_ms = matches
        .get_one::<u64>("circuit-cooldown-ms")
        .copied()
        .unwrap_or(5_000);

    Ok(Config {
        stdio,
        sse,
        streamable_http,
        output_transport,
        port,
        base_url,
        sse_path,
        message_path,
        streamable_http_path,
        log_level,
        cors,
        health_endpoints,
        headers,
        env,
        stateful,
        session_timeout,
        protocol_version,
        runtime_prompt,
        runtime_admin_port,
        runtime_admin_host,
        runtime_admin_token,
        retry_attempts,
        retry_base_delay_ms,
        retry_max_delay_ms,
        circuit_failure_threshold,
        circuit_cooldown_ms,
    })
}

fn parse_generate_config_from(raw_args: Vec<String>) -> Result<GenerateConfig, ConfigError> {
    let matches = build_generate_root_cli().get_matches_from(raw_args);
    let Some(sub) = matches.subcommand_matches("generate") else {
        return Err(ConfigError::InvalidArg(
            "Failed to parse generate subcommand".to_string(),
        ));
    };

    let definition = PathBuf::from(required_arg(sub, "definition")?);
    let out = PathBuf::from(required_arg(sub, "out")?);
    let server = sub.get_one::<String>("server").cloned();
    let artifact_name = sub.get_one::<String>("artifact-name").cloned();
    let mcpway_binary = sub.get_one::<String>("mcpway-binary").map(PathBuf::from);

    let bundle_mcpway = !sub.get_flag("no-bundle-mcpway");
    let compile_wrapper = !sub.get_flag("no-compile-wrapper");

    Ok(GenerateConfig {
        definition,
        server,
        out,
        artifact_name,
        bundle_mcpway,
        mcpway_binary,
        compile_wrapper,
    })
}

fn parse_regenerate_config_from(raw_args: Vec<String>) -> Result<RegenerateConfig, ConfigError> {
    let matches = build_regenerate_root_cli().get_matches_from(raw_args);
    let Some(sub) = matches.subcommand_matches("regenerate") else {
        return Err(ConfigError::InvalidArg(
            "Failed to parse regenerate subcommand".to_string(),
        ));
    };

    let metadata = PathBuf::from(required_arg(sub, "metadata")?);
    let definition = sub.get_one::<String>("definition").map(PathBuf::from);
    let server = sub.get_one::<String>("server").cloned();
    let out = sub.get_one::<String>("out").map(PathBuf::from);
    let mcpway_binary = sub.get_one::<String>("mcpway-binary").map(PathBuf::from);

    Ok(RegenerateConfig {
        metadata,
        definition,
        server,
        out,
        bundle_mcpway: parse_optional_bool(sub, "bundle-mcpway", "no-bundle-mcpway"),
        mcpway_binary,
        compile_wrapper: parse_optional_bool(sub, "compile-wrapper", "no-compile-wrapper"),
    })
}

fn parse_connect_config_from(raw_args: Vec<String>) -> Result<ConnectConfig, ConfigError> {
    let matches = build_connect_root_cli().get_matches_from(raw_args);
    let Some(sub) = matches.subcommand_matches("connect") else {
        return Err(ConfigError::InvalidArg(
            "Failed to parse connect subcommand".to_string(),
        ));
    };

    let endpoint = sub.get_one::<String>("endpoint").cloned();
    let server = sub.get_one::<String>("server").cloned();
    let stdio_cmd = sub.get_one::<String>("stdio-cmd").cloned();
    let stdio_wrapper = sub.get_one::<String>("stdio-wrapper").map(PathBuf::from);
    let stdio_mode = stdio_cmd.is_some() || stdio_wrapper.is_some();
    let selected_modes = endpoint.is_some() as u8 + server.is_some() as u8 + stdio_mode as u8;

    if selected_modes == 0 {
        return Err(ConfigError::InvalidArg(
            "connect requires one mode: <ENDPOINT>, --server <NAME>, --stdio-cmd <CMD>, or --stdio-wrapper <PATH>".to_string(),
        ));
    }
    if selected_modes > 1 {
        return Err(ConfigError::InvalidArg(
            "connect accepts exactly one mode: endpoint, --server, or stdio override".to_string(),
        ));
    }
    if stdio_cmd.is_some() && stdio_wrapper.is_some() {
        return Err(ConfigError::InvalidArg(
            "--stdio-cmd and --stdio-wrapper cannot be used together".to_string(),
        ));
    }

    let protocol = sub.get_one::<ConnectProtocol>("protocol").copied();
    if protocol.is_some() && stdio_mode {
        return Err(ConfigError::InvalidArg(
            "--protocol is only supported for remote endpoint/server modes".to_string(),
        ));
    }
    let header_values: Vec<String> = sub
        .get_many::<String>("header")
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();
    let stdio_arg_values: Vec<String> = sub
        .get_many::<String>("stdio-arg")
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();
    let stdio_env_values: Vec<String> = sub
        .get_many::<String>("stdio-env")
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();
    let oauth2_bearer = sub.get_one::<String>("oauth2-bearer").cloned();
    let headers = parse_headers(&header_values, oauth2_bearer.as_deref())?;
    let stdio_env = parse_env_values(&stdio_env_values);

    let registry_path = sub.get_one::<String>("registry").map(PathBuf::from);
    let save_profile_dir = sub.get_one::<String>("save-profile").map(PathBuf::from);
    let profile_name = sub.get_one::<String>("profile-name").cloned();
    let save_wrapper_dir = sub.get_one::<String>("save-wrapper").map(PathBuf::from);
    let log_level = sub
        .get_one::<LogLevel>("log-level")
        .copied()
        .unwrap_or(LogLevel::Info);
    let protocol_version = sub
        .get_one::<String>("protocol-version")
        .cloned()
        .unwrap_or_else(|| "2024-11-05".to_string());
    let retry_attempts = sub.get_one::<u32>("retry-attempts").copied().unwrap_or(2);
    let retry_base_delay_ms = sub
        .get_one::<u64>("retry-base-delay-ms")
        .copied()
        .unwrap_or(250);
    let retry_max_delay_ms = sub
        .get_one::<u64>("retry-max-delay-ms")
        .copied()
        .unwrap_or(2_000);
    let circuit_failure_threshold = sub
        .get_one::<u32>("circuit-failure-threshold")
        .copied()
        .unwrap_or(3);
    let circuit_cooldown_ms = sub
        .get_one::<u64>("circuit-cooldown-ms")
        .copied()
        .unwrap_or(5_000);

    let oauth_scopes: Vec<String> = sub
        .get_many::<String>("oauth-scope")
        .map(|vals| vals.map(|v| v.to_string()).collect())
        .unwrap_or_default();
    let oauth_profile = sub.get_one::<String>("oauth-profile").cloned();
    let oauth_issuer = sub.get_one::<String>("oauth-issuer").cloned();
    let oauth_client_id = sub.get_one::<String>("oauth-client-id").cloned();
    let oauth_flow = sub
        .get_one::<OAuthFlow>("oauth-flow")
        .copied()
        .unwrap_or(OAuthFlow::Device);
    let oauth_no_browser = sub.get_flag("oauth-no-browser");
    let oauth_cache = sub.get_one::<String>("oauth-cache").map(PathBuf::from);
    let oauth_login = sub.get_flag("oauth-login");
    let oauth_logout = sub.get_flag("oauth-logout");
    let oauth_audience = sub.get_one::<String>("oauth-audience").cloned();
    let oauth_requested = oauth_profile.is_some()
        || oauth_issuer.is_some()
        || oauth_client_id.is_some()
        || !oauth_scopes.is_empty()
        || oauth_no_browser
        || oauth_cache.is_some()
        || oauth_login
        || oauth_logout
        || oauth_audience.is_some();

    let oauth = if oauth_requested {
        let issuer = oauth_issuer.ok_or_else(|| {
            ConfigError::InvalidArg("--oauth-issuer is required when OAuth is enabled".to_string())
        })?;
        let client_id = oauth_client_id.ok_or_else(|| {
            ConfigError::InvalidArg(
                "--oauth-client-id is required when OAuth is enabled".to_string(),
            )
        })?;
        Some(ConnectOauthConfig {
            profile: oauth_profile,
            issuer,
            client_id,
            scopes: oauth_scopes,
            flow: oauth_flow,
            no_browser: oauth_no_browser,
            cache_path: oauth_cache,
            login: oauth_login,
            logout: oauth_logout,
            audience: oauth_audience,
        })
    } else {
        None
    };

    Ok(ConnectConfig {
        endpoint,
        server,
        stdio_cmd,
        stdio_args: stdio_arg_values,
        stdio_env,
        stdio_wrapper,
        save_wrapper_dir,
        protocol,
        headers,
        registry_path,
        save_profile_dir,
        profile_name,
        log_level,
        protocol_version,
        oauth,
        retry_attempts,
        retry_base_delay_ms,
        retry_max_delay_ms,
        circuit_failure_threshold,
        circuit_cooldown_ms,
    })
}

fn parse_discover_config_from(raw_args: Vec<String>) -> Result<DiscoverConfig, ConfigError> {
    let matches = build_discover_root_cli().get_matches_from(raw_args);
    let Some(sub) = matches.subcommand_matches("discover") else {
        return Err(ConfigError::InvalidArg(
            "Failed to parse discover subcommand".to_string(),
        ));
    };

    let from = sub
        .get_one::<ImportSource>("from")
        .copied()
        .unwrap_or(ImportSource::Auto);
    let project_root = sub.get_one::<String>("project-root").map(PathBuf::from);
    let print_json = sub.get_flag("print-json");
    let strict_conflicts = sub.get_flag("strict-conflicts");
    let search = sub.get_one::<String>("search").cloned();
    let transport = sub.get_one::<DiscoverTransport>("transport").copied();
    let scope = sub.get_one::<DiscoverScope>("scope").copied();
    let enabled_only = sub.get_flag("enabled-only");
    let sort_by = sub
        .get_one::<DiscoverSortBy>("sort")
        .copied()
        .unwrap_or(DiscoverSortBy::Name);
    let order = sub
        .get_one::<SortOrder>("order")
        .copied()
        .unwrap_or(SortOrder::Asc);
    let offset = sub.get_one::<usize>("offset").copied().unwrap_or(0);
    let limit = sub.get_one::<usize>("limit").copied();

    Ok(DiscoverConfig {
        from,
        project_root,
        print_json,
        strict_conflicts,
        search,
        transport,
        scope,
        enabled_only,
        sort_by,
        order,
        offset,
        limit,
    })
}

fn parse_import_config_from(raw_args: Vec<String>) -> Result<ImportConfig, ConfigError> {
    let matches = build_import_root_cli().get_matches_from(raw_args);
    let Some(sub) = matches.subcommand_matches("import") else {
        return Err(ConfigError::InvalidArg(
            "Failed to parse import subcommand".to_string(),
        ));
    };

    let from = sub
        .get_one::<ImportSource>("from")
        .copied()
        .unwrap_or(ImportSource::Auto);
    let project_root = sub.get_one::<String>("project-root").map(PathBuf::from);
    let print_json = sub.get_flag("print-json");
    let strict_conflicts = sub.get_flag("strict-conflicts");
    let registry_path = sub.get_one::<String>("registry").map(PathBuf::from);
    let save_profiles_dir = sub.get_one::<String>("save-profiles").map(PathBuf::from);
    let bundle_mcpway = sub.get_flag("bundle-mcpway");
    let compile_wrapper = sub.get_flag("compile-wrapper");

    Ok(ImportConfig {
        from,
        project_root,
        print_json,
        strict_conflicts,
        registry_path,
        save_profiles_dir,
        bundle_mcpway,
        compile_wrapper,
    })
}

fn parse_logs_config_from(raw_args: Vec<String>) -> Result<LogsConfig, ConfigError> {
    let matches = build_logs_root_cli().get_matches_from(raw_args);
    let Some(sub) = matches.subcommand_matches("logs") else {
        return Err(ConfigError::InvalidArg(
            "Failed to parse logs command".to_string(),
        ));
    };
    let Some(tail) = sub.subcommand_matches("tail") else {
        return Err(ConfigError::InvalidArg(
            "logs currently supports only the 'tail' subcommand".to_string(),
        ));
    };

    let file = tail.get_one::<String>("file").map(PathBuf::from);
    let lines = tail.get_one::<usize>("lines").copied().unwrap_or(200);
    let level = tail.get_one::<LogsLevel>("level").copied();
    let transport = tail.get_one::<LogsTransport>("transport").copied();
    let json = tail.get_flag("json");
    let no_follow = tail.get_flag("no-follow");
    let follow = !no_follow;

    Ok(LogsConfig::Tail(LogsTailConfig {
        file,
        follow,
        lines,
        level,
        transport,
        json,
    }))
}

fn build_cli() -> Command {
    Command::new("mcpway")
        .arg(Arg::new("stdio").long("stdio").value_name("CMD"))
        .arg(Arg::new("sse").long("sse").value_name("URL"))
        .arg(
            Arg::new("streamable-http")
                .long("streamable-http")
                .value_name("URL"),
        )
        .arg(
            Arg::new("output-transport")
                .long("output-transport")
                .value_parser(clap::builder::EnumValueParser::<OutputTransport>::new())
                .value_name("stdio|sse|ws|streamable-http"),
        )
        .arg(Arg::new("port").long("port").value_name("PORT"))
        .arg(
            Arg::new("base-url")
                .long("base-url")
                .value_name("URL")
                .default_value(""),
        )
        .arg(
            Arg::new("sse-path")
                .long("sse-path")
                .value_name("PATH")
                .default_value("/sse"),
        )
        .arg(
            Arg::new("message-path")
                .long("message-path")
                .value_name("PATH")
                .default_value("/message"),
        )
        .arg(
            Arg::new("streamable-http-path")
                .long("streamable-http-path")
                .value_name("PATH")
                .default_value("/mcp"),
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_parser(clap::builder::EnumValueParser::<LogLevel>::new())
                .default_value("info"),
        )
        .arg(
            Arg::new("cors")
                .long("cors")
                .num_args(0..=1)
                .action(ArgAction::Append)
                .value_name("ORIGIN"),
        )
        .arg(
            Arg::new("health-endpoint")
                .long("health-endpoint")
                .action(ArgAction::Append)
                .value_name("PATH"),
        )
        .arg(
            Arg::new("header")
                .long("header")
                .action(ArgAction::Append)
                .value_name("HEADER"),
        )
        .arg(
            Arg::new("env")
                .long("env")
                .action(ArgAction::Append)
                .value_name("KEY=VALUE"),
        )
        .arg(
            Arg::new("oauth2-bearer")
                .long("oauth2-bearer")
                .value_name("TOKEN"),
        )
        .arg(
            Arg::new("stateful")
                .long("stateful")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("session-timeout")
                .long("session-timeout")
                .value_name("MILLISECONDS"),
        )
        .arg(
            Arg::new("protocol-version")
                .long("protocol-version")
                .default_value("2024-11-05"),
        )
        .arg(
            Arg::new("runtime-prompt")
                .long("runtime-prompt")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("runtime-admin-port")
                .long("runtime-admin-port")
                .value_name("PORT"),
        )
        .arg(
            Arg::new("runtime-admin-host")
                .long("runtime-admin-host")
                .value_name("HOST")
                .default_value("127.0.0.1"),
        )
        .arg(
            Arg::new("runtime-admin-token")
                .long("runtime-admin-token")
                .value_name("TOKEN"),
        )
        .arg(
            Arg::new("retry-attempts")
                .long("retry-attempts")
                .value_parser(clap::value_parser!(u32))
                .value_name("N")
                .default_value("2"),
        )
        .arg(
            Arg::new("retry-base-delay-ms")
                .long("retry-base-delay-ms")
                .value_parser(clap::value_parser!(u64).range(1..))
                .value_name("MILLISECONDS")
                .default_value("250"),
        )
        .arg(
            Arg::new("retry-max-delay-ms")
                .long("retry-max-delay-ms")
                .value_parser(clap::value_parser!(u64).range(1..))
                .value_name("MILLISECONDS")
                .default_value("2000"),
        )
        .arg(
            Arg::new("circuit-failure-threshold")
                .long("circuit-failure-threshold")
                .value_parser(clap::value_parser!(u32).range(1..))
                .value_name("N")
                .default_value("3"),
        )
        .arg(
            Arg::new("circuit-cooldown-ms")
                .long("circuit-cooldown-ms")
                .value_parser(clap::value_parser!(u64).range(1..))
                .value_name("MILLISECONDS")
                .default_value("5000"),
        )
}

fn build_generate_root_cli() -> Command {
    Command::new("mcpway")
        .subcommand_required(true)
        .subcommand(build_generate_subcommand())
}

fn build_regenerate_root_cli() -> Command {
    Command::new("mcpway")
        .subcommand_required(true)
        .subcommand(build_regenerate_subcommand())
}

fn build_connect_root_cli() -> Command {
    Command::new("mcpway")
        .subcommand_required(true)
        .subcommand(build_connect_subcommand())
}

fn build_discover_root_cli() -> Command {
    Command::new("mcpway")
        .subcommand_required(true)
        .subcommand(build_discover_subcommand())
}

fn build_import_root_cli() -> Command {
    Command::new("mcpway")
        .subcommand_required(true)
        .subcommand(build_import_subcommand())
}

fn build_logs_root_cli() -> Command {
    Command::new("mcpway")
        .subcommand_required(true)
        .subcommand(build_logs_subcommand())
}

fn build_generate_subcommand() -> Command {
    Command::new("generate")
        .about("Generate runnable artifacts from an MCP server definition")
        .arg(
            Arg::new("definition")
                .long("definition")
                .required(true)
                .value_name("PATH"),
        )
        .arg(Arg::new("server").long("server").value_name("NAME"))
        .arg(Arg::new("out").long("out").required(true).value_name("DIR"))
        .arg(
            Arg::new("artifact-name")
                .long("artifact-name")
                .value_name("NAME"),
        )
        .arg(
            Arg::new("bundle-mcpway")
                .long("bundle-mcpway")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-bundle-mcpway"),
        )
        .arg(
            Arg::new("no-bundle-mcpway")
                .long("no-bundle-mcpway")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("mcpway-binary")
                .long("mcpway-binary")
                .value_name("PATH"),
        )
        .arg(
            Arg::new("compile-wrapper")
                .long("compile-wrapper")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-compile-wrapper"),
        )
        .arg(
            Arg::new("no-compile-wrapper")
                .long("no-compile-wrapper")
                .action(ArgAction::SetTrue),
        )
}

fn build_regenerate_subcommand() -> Command {
    Command::new("regenerate")
        .about("Regenerate artifacts from mcpway metadata")
        .arg(
            Arg::new("metadata")
                .long("metadata")
                .required(true)
                .value_name("PATH"),
        )
        .arg(Arg::new("definition").long("definition").value_name("PATH"))
        .arg(Arg::new("server").long("server").value_name("NAME"))
        .arg(Arg::new("out").long("out").value_name("DIR"))
        .arg(
            Arg::new("bundle-mcpway")
                .long("bundle-mcpway")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-bundle-mcpway"),
        )
        .arg(
            Arg::new("no-bundle-mcpway")
                .long("no-bundle-mcpway")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("mcpway-binary")
                .long("mcpway-binary")
                .value_name("PATH"),
        )
        .arg(
            Arg::new("compile-wrapper")
                .long("compile-wrapper")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-compile-wrapper"),
        )
        .arg(
            Arg::new("no-compile-wrapper")
                .long("no-compile-wrapper")
                .action(ArgAction::SetTrue),
        )
}

fn build_connect_subcommand() -> Command {
    Command::new("connect")
        .about("Connect to an MCP endpoint/imported server or run stdio overrides")
        .arg(
            Arg::new("endpoint")
                .value_name("ENDPOINT")
                .help("Endpoint URL (ws/wss/http/https)"),
        )
        .arg(
            Arg::new("server")
                .long("server")
                .value_name("NAME")
                .conflicts_with("endpoint")
                .help("Imported server name from registry"),
        )
        .arg(
            Arg::new("stdio-cmd")
                .long("stdio-cmd")
                .value_name("CMD")
                .help("Run an ad-hoc stdio command from connect mode"),
        )
        .arg(
            Arg::new("stdio-arg")
                .long("stdio-arg")
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .value_name("ARG")
                .help("Additional stdio argument (repeatable)"),
        )
        .arg(
            Arg::new("stdio-env")
                .long("stdio-env")
                .action(ArgAction::Append)
                .value_name("KEY=VALUE")
                .help("Additional stdio env var (repeatable)"),
        )
        .arg(
            Arg::new("stdio-wrapper")
                .long("stdio-wrapper")
                .value_name("PATH")
                .help("Load stdio command/args/env from wrapper metadata path"),
        )
        .arg(
            Arg::new("save-wrapper")
                .long("save-wrapper")
                .value_name("DIR")
                .help("Persist resolved stdio wrapper config to a directory"),
        )
        .arg(
            Arg::new("protocol")
                .long("protocol")
                .value_parser(clap::builder::EnumValueParser::<ConnectProtocol>::new())
                .value_name("sse|streamable-http|ws"),
        )
        .arg(
            Arg::new("header")
                .long("header")
                .action(ArgAction::Append)
                .value_name("HEADER"),
        )
        .arg(
            Arg::new("oauth2-bearer")
                .long("oauth2-bearer")
                .value_name("TOKEN"),
        )
        .arg(
            Arg::new("oauth-profile")
                .long("oauth-profile")
                .value_name("NAME"),
        )
        .arg(
            Arg::new("oauth-issuer")
                .long("oauth-issuer")
                .value_name("URL"),
        )
        .arg(
            Arg::new("oauth-client-id")
                .long("oauth-client-id")
                .value_name("ID"),
        )
        .arg(
            Arg::new("oauth-scope")
                .long("oauth-scope")
                .action(ArgAction::Append)
                .value_name("SCOPE"),
        )
        .arg(
            Arg::new("oauth-flow")
                .long("oauth-flow")
                .value_parser(clap::builder::EnumValueParser::<OAuthFlow>::new())
                .default_value("device")
                .value_name("device|auth-code"),
        )
        .arg(
            Arg::new("oauth-no-browser")
                .long("oauth-no-browser")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("oauth-cache")
                .long("oauth-cache")
                .value_name("PATH"),
        )
        .arg(
            Arg::new("oauth-login")
                .long("oauth-login")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("oauth-logout")
                .long("oauth-logout")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("oauth-audience")
                .long("oauth-audience")
                .value_name("AUDIENCE"),
        )
        .arg(
            Arg::new("save-profile")
                .long("save-profile")
                .value_name("DIR"),
        )
        .arg(Arg::new("registry").long("registry").value_name("PATH"))
        .arg(
            Arg::new("profile-name")
                .long("profile-name")
                .value_name("NAME"),
        )
        .arg(
            Arg::new("retry-attempts")
                .long("retry-attempts")
                .value_parser(clap::value_parser!(u32))
                .value_name("N")
                .default_value("2"),
        )
        .arg(
            Arg::new("retry-base-delay-ms")
                .long("retry-base-delay-ms")
                .value_parser(clap::value_parser!(u64).range(1..))
                .value_name("MILLISECONDS")
                .default_value("250"),
        )
        .arg(
            Arg::new("retry-max-delay-ms")
                .long("retry-max-delay-ms")
                .value_parser(clap::value_parser!(u64).range(1..))
                .value_name("MILLISECONDS")
                .default_value("2000"),
        )
        .arg(
            Arg::new("circuit-failure-threshold")
                .long("circuit-failure-threshold")
                .value_parser(clap::value_parser!(u32).range(1..))
                .value_name("N")
                .default_value("3"),
        )
        .arg(
            Arg::new("circuit-cooldown-ms")
                .long("circuit-cooldown-ms")
                .value_parser(clap::value_parser!(u64).range(1..))
                .value_name("MILLISECONDS")
                .default_value("5000"),
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_parser(clap::builder::EnumValueParser::<LogLevel>::new())
                .default_value("info"),
        )
        .arg(
            Arg::new("protocol-version")
                .long("protocol-version")
                .default_value("2024-11-05"),
        )
}

fn build_discover_subcommand() -> Command {
    Command::new("discover")
        .about("Discover MCP server definitions from local tool configs")
        .arg(
            Arg::new("from")
                .long("from")
                .value_parser(clap::builder::EnumValueParser::<ImportSource>::new())
                .value_name("auto|cursor|claude|codex|windsurf|opencode|nodecode|vscode"),
        )
        .arg(
            Arg::new("project-root")
                .long("project-root")
                .value_name("DIR"),
        )
        .arg(
            Arg::new("print-json")
                .long("json")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("strict-conflicts")
                .long("strict-conflicts")
                .action(ArgAction::SetTrue),
        )
        .arg(Arg::new("search").long("search").value_name("QUERY"))
        .arg(
            Arg::new("transport")
                .long("transport")
                .value_parser(clap::builder::EnumValueParser::<DiscoverTransport>::new())
                .value_name("stdio|sse|ws|streamable-http"),
        )
        .arg(
            Arg::new("scope")
                .long("scope")
                .value_parser(clap::builder::EnumValueParser::<DiscoverScope>::new())
                .value_name("project|global"),
        )
        .arg(
            Arg::new("enabled-only")
                .long("enabled-only")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("sort")
                .long("sort")
                .value_parser(clap::builder::EnumValueParser::<DiscoverSortBy>::new())
                .value_name("name|source|scope|transport|origin-path")
                .default_value("name"),
        )
        .arg(
            Arg::new("order")
                .long("order")
                .value_parser(clap::builder::EnumValueParser::<SortOrder>::new())
                .value_name("asc|desc")
                .default_value("asc"),
        )
        .arg(
            Arg::new("offset")
                .long("offset")
                .value_parser(clap::value_parser!(usize))
                .value_name("N")
                .default_value("0"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_parser(clap::value_parser!(usize))
                .value_name("N"),
        )
}

fn build_import_subcommand() -> Command {
    Command::new("import")
        .about("Discover and import MCP servers into mcpway registry")
        .arg(
            Arg::new("from")
                .long("from")
                .value_parser(clap::builder::EnumValueParser::<ImportSource>::new())
                .value_name("auto|cursor|claude|codex|windsurf|opencode|nodecode|vscode"),
        )
        .arg(
            Arg::new("project-root")
                .long("project-root")
                .value_name("DIR"),
        )
        .arg(
            Arg::new("print-json")
                .long("json")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("strict-conflicts")
                .long("strict-conflicts")
                .action(ArgAction::SetTrue),
        )
        .arg(Arg::new("registry").long("registry").value_name("PATH"))
        .arg(
            Arg::new("save-profiles")
                .long("save-profiles")
                .value_name("DIR"),
        )
        .arg(
            Arg::new("bundle-mcpway")
                .long("bundle-mcpway")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("compile-wrapper")
                .long("compile-wrapper")
                .action(ArgAction::SetTrue),
        )
}

fn build_logs_subcommand() -> Command {
    Command::new("logs")
        .about("Read and tail local mcpway logs")
        .subcommand_required(true)
        .subcommand(
            Command::new("tail")
                .about("Tail local log file output")
                .arg(Arg::new("file").long("file").value_name("PATH"))
                .arg(
                    Arg::new("lines")
                        .long("lines")
                        .value_name("N")
                        .value_parser(clap::value_parser!(usize))
                        .default_value("200"),
                )
                .arg(
                    Arg::new("level")
                        .long("level")
                        .value_parser(clap::builder::EnumValueParser::<LogsLevel>::new())
                        .value_name("debug|info|warn|error"),
                )
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .value_parser(clap::builder::EnumValueParser::<LogsTransport>::new())
                        .value_name("stdio|sse|ws|streamable-http|connect"),
                )
                .arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
                .arg(
                    Arg::new("no-follow")
                        .long("no-follow")
                        .action(ArgAction::SetTrue),
                ),
        )
}

fn required_arg<'a>(matches: &'a ArgMatches, key: &str) -> Result<&'a str, ConfigError> {
    matches
        .get_one::<String>(key)
        .map(String::as_str)
        .ok_or_else(|| ConfigError::InvalidArg(format!("Missing required arg --{key}")))
}

fn parse_optional_bool(matches: &ArgMatches, enabled: &str, disabled: &str) -> Option<bool> {
    if matches.get_flag(enabled) {
        Some(true)
    } else if matches.get_flag(disabled) {
        Some(false)
    } else {
        None
    }
}

fn cli_help_text() -> String {
    let mut command = build_cli();
    let mut bytes = Vec::new();
    if command.write_long_help(&mut bytes).is_ok() {
        return String::from_utf8(bytes).unwrap_or_else(|_| "Usage: mcpway [OPTIONS]".into());
    }
    "Usage: mcpway [OPTIONS]".into()
}

fn no_args_banner_text() -> String {
    no_args_banner_text_with_style(should_use_ansi_styling())
}

fn no_args_banner_text_with_style(use_ansi: bool) -> String {
    let mut output = String::new();
    output.push_str(&format!("{}\n", maybe_bold("MCPway CLI", use_ansi)));
    output.push_str("No input transport provided. Choose one input mode:\n");
    output.push_str("  --stdio <CMD>, --sse <URL>, or --streamable-http <URL>\n\n");
    output.push_str(&format!(
        "{}\n",
        maybe_bold("Generator Subcommands", use_ansi)
    ));
    output.push_str(
        "  mcpway generate --definition ./servers.json --server myServer --out ./artifact\n",
    );
    output.push_str("  mcpway regenerate --metadata ./artifact/mcpway-artifact.json\n\n");
    output.push_str(&format!("{}\n", maybe_bold("Ad-hoc Connect", use_ansi)));
    output.push_str("  mcpway connect https://example.com/mcp\n");
    output.push_str("  mcpway connect wss://example.com/ws --protocol ws\n\n");
    output.push_str(&format!(
        "{}\n",
        maybe_bold("Zero-Config Discovery", use_ansi)
    ));
    output.push_str("  mcpway discover --from auto\n");
    output.push_str("  mcpway import --from auto --save-profiles ./profiles\n\n");
    output.push_str(&format!("{}\n", maybe_bold("Quick Start", use_ansi)));
    output.push_str("  mcpway --stdio \"npx -y @modelcontextprotocol/server-everything\"\n");
    output.push_str("  mcpway --sse http://127.0.0.1:9000/sse\n");
    output.push_str("  mcpway --streamable-http http://127.0.0.1:9000/mcp\n\n");
    output.push_str(&format!("{}\n", maybe_bold("Full Options", use_ansi)));
    output.push_str(&cli_help_text());
    output
}

fn should_use_ansi_styling() -> bool {
    std::io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none()
}

fn maybe_bold(text: &str, use_ansi: bool) -> String {
    if use_ansi {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn default_output_transport(args: &[String]) -> Option<OutputTransport> {
    if args.iter().any(|arg| arg == "--stdio") {
        return Some(OutputTransport::Sse);
    }
    if args.iter().any(|arg| arg == "--sse") {
        return Some(OutputTransport::Stdio);
    }
    if args.iter().any(|arg| arg == "--streamable-http") {
        return Some(OutputTransport::Stdio);
    }
    None
}

#[derive(Default)]
struct CorsInput {
    present: bool,
    allow_all: bool,
    values: Vec<String>,
}

fn parse_cors_flags(args: &[String]) -> CorsInput {
    let mut input = CorsInput::default();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--cors" {
            input.present = true;
            let next = args.get(i + 1);
            if let Some(next_val) = next {
                if next_val.starts_with("--") {
                    input.allow_all = true;
                } else {
                    if next_val == "*" {
                        input.allow_all = true;
                    } else {
                        input.values.push(next_val.clone());
                    }
                    i += 1;
                }
            } else {
                input.allow_all = true;
            }
        }
        i += 1;
    }
    input
}

fn parse_headers(
    header_values: &[String],
    oauth2_bearer: Option<&str>,
) -> Result<HeadersMap, ConfigError> {
    let mut headers: HashMap<String, String> = HashMap::new();
    for raw in header_values {
        let Some((key, value)) = raw.split_once(':') else {
            tracing::error!("Invalid header format: {raw}, ignoring");
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            tracing::error!("Invalid header format: {raw}, ignoring");
            continue;
        }
        headers.insert(key.to_string(), value.to_string());
    }
    if let Some(token) = oauth2_bearer {
        let token = token.trim();
        if !token.is_empty() {
            headers.insert("Authorization".to_string(), format!("Bearer {token}"));
        }
    }
    Ok(headers)
}

fn parse_env_values(values: &[String]) -> HashMap<String, String> {
    let mut env_map = HashMap::new();
    for raw in values {
        let Some((key, value)) = raw.split_once('=') else {
            tracing::error!("Invalid env format: {raw}, expected KEY=VALUE, ignoring");
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            tracing::error!("Invalid env format: {raw}, expected KEY=VALUE, ignoring");
            continue;
        }
        env_map.insert(key.to_string(), value.to_string());
    }
    env_map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, ConfigError> {
        parse_config_from(args.iter().map(|arg| arg.to_string()).collect())
    }

    fn parse_cli(args: &[&str]) -> Result<CliCommand, ConfigError> {
        parse_cli_command_from(args.iter().map(|arg| arg.to_string()).collect())
    }

    #[test]
    fn parse_requires_one_transport() {
        let err = parse(&["mcpway"]).expect_err("expected missing transport");
        assert!(matches!(err, ConfigError::MissingTransport));
    }

    #[test]
    fn parse_rejects_multiple_transports() {
        let err = parse(&[
            "mcpway",
            "--stdio",
            "cat",
            "--sse",
            "http://127.0.0.1:9000/sse",
        ])
        .expect_err("expected multiple transports error");
        assert!(matches!(err, ConfigError::MultipleTransports));
    }

    #[test]
    fn parse_infers_default_outputs() {
        let stdio_cfg = parse(&["mcpway", "--stdio", "cat"]).expect("stdio parse failed");
        assert_eq!(stdio_cfg.output_transport, OutputTransport::Sse);

        let sse_cfg =
            parse(&["mcpway", "--sse", "http://127.0.0.1:9000/sse"]).expect("sse parse failed");
        assert_eq!(sse_cfg.output_transport, OutputTransport::Stdio);

        let streamable_cfg = parse(&["mcpway", "--streamable-http", "http://127.0.0.1:9000/mcp"])
            .expect("streamable parse failed");
        assert_eq!(streamable_cfg.output_transport, OutputTransport::Stdio);
    }

    #[test]
    fn parse_accepts_streamable_http_kebab_case() {
        let cfg = parse(&[
            "mcpway",
            "--stdio",
            "cat",
            "--output-transport",
            "streamable-http",
        ])
        .expect("streamable-http should parse");
        assert_eq!(cfg.output_transport, OutputTransport::StreamableHttp);
    }

    #[test]
    fn output_transport_value_enum_rejects_invalid_variant() {
        assert_eq!(
            OutputTransport::from_str("streamable-http", true).ok(),
            Some(OutputTransport::StreamableHttp)
        );
        assert!(OutputTransport::from_str("streamableHttp", true).is_err());
        assert!(OutputTransport::from_str("streamable_http", true).is_err());
    }

    #[test]
    fn parse_rejects_non_positive_session_timeout() {
        let err = parse(&["mcpway", "--stdio", "cat", "--session-timeout", "0"])
            .expect_err("expected invalid session timeout");
        match err {
            ConfigError::InvalidSessionTimeout(message) => {
                assert!(message.contains("session-timeout must be a positive number"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_invalid_runtime_admin_port() {
        let err = parse(&["mcpway", "--stdio", "cat", "--runtime-admin-port", "70000"])
            .expect_err("expected invalid runtime admin port");
        match err {
            ConfigError::InvalidRuntimePort(message) => {
                assert!(message.contains("runtime-admin-port must be in 1..=65535"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn parse_reads_runtime_admin_host_and_token() {
        let cfg = parse(&[
            "mcpway",
            "--stdio",
            "cat",
            "--runtime-admin-port",
            "9101",
            "--runtime-admin-host",
            "0.0.0.0",
            "--runtime-admin-token",
            "abc-token",
        ])
        .expect("runtime admin args should parse");

        assert_eq!(cfg.runtime_admin_port, Some(9101));
        assert_eq!(cfg.runtime_admin_host, "0.0.0.0");
        assert_eq!(cfg.runtime_admin_token.as_deref(), Some("abc-token"));
    }

    #[test]
    fn parse_reads_env_values() {
        let cfg = parse(&[
            "mcpway",
            "--stdio",
            "cat",
            "--env",
            "API_KEY=abc123",
            "--env",
            "MCP_MODE=debug",
        ])
        .expect("env parse failed");

        assert_eq!(cfg.env.get("API_KEY"), Some(&"abc123".to_string()));
        assert_eq!(cfg.env.get("MCP_MODE"), Some(&"debug".to_string()));
    }

    #[test]
    fn parse_generate_subcommand_defaults() {
        let cmd = parse_cli(&[
            "mcpway",
            "generate",
            "--definition",
            "./servers.json",
            "--out",
            "./artifact",
        ])
        .expect("generate parse failed");

        match cmd {
            CliCommand::Generate(cfg) => {
                assert_eq!(cfg.definition, PathBuf::from("./servers.json"));
                assert_eq!(cfg.out, PathBuf::from("./artifact"));
                assert!(cfg.bundle_mcpway);
                assert!(cfg.compile_wrapper);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_regenerate_subcommand_optional_flags() {
        let cmd = parse_cli(&[
            "mcpway",
            "regenerate",
            "--metadata",
            "./artifact/mcpway-artifact.json",
            "--no-compile-wrapper",
        ])
        .expect("regenerate parse failed");

        match cmd {
            CliCommand::Regenerate(cfg) => {
                assert_eq!(
                    cfg.metadata,
                    PathBuf::from("./artifact/mcpway-artifact.json")
                );
                assert_eq!(cfg.compile_wrapper, Some(false));
                assert_eq!(cfg.bundle_mcpway, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_connect_subcommand_defaults() {
        let cmd = parse_cli(&["mcpway", "connect", "https://example.com/mcp"])
            .expect("connect parse failed");

        match cmd {
            CliCommand::Connect(cfg) => {
                assert_eq!(cfg.endpoint, Some("https://example.com/mcp".to_string()));
                assert_eq!(cfg.server, None);
                assert_eq!(cfg.protocol, None);
                assert!(cfg.headers.is_empty());
                assert_eq!(cfg.save_profile_dir, None);
                assert_eq!(cfg.profile_name, None);
                assert_eq!(cfg.retry_attempts, 2);
                assert_eq!(cfg.circuit_failure_threshold, 3);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_connect_subcommand_with_options() {
        let cmd = parse_cli(&[
            "mcpway",
            "connect",
            "wss://example.com/ws",
            "--protocol",
            "ws",
            "--header",
            "X-Test: abc",
            "--save-profile",
            "./profile",
            "--profile-name",
            "my-conn",
        ])
        .expect("connect parse failed");

        match cmd {
            CliCommand::Connect(cfg) => {
                assert_eq!(cfg.endpoint, Some("wss://example.com/ws".to_string()));
                assert_eq!(cfg.server, None);
                assert_eq!(cfg.protocol, Some(ConnectProtocol::Ws));
                assert_eq!(cfg.headers.get("X-Test"), Some(&"abc".to_string()));
                assert_eq!(cfg.save_profile_dir, Some(PathBuf::from("./profile")));
                assert_eq!(cfg.profile_name, Some("my-conn".to_string()));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_connect_subcommand_with_server_mode() {
        let cmd = parse_cli(&[
            "mcpway",
            "connect",
            "--server",
            "github",
            "--registry",
            "./imported.json",
        ])
        .expect("connect parse failed");

        match cmd {
            CliCommand::Connect(cfg) => {
                assert_eq!(cfg.endpoint, None);
                assert_eq!(cfg.server, Some("github".to_string()));
                assert_eq!(cfg.registry_path, Some(PathBuf::from("./imported.json")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_connect_subcommand_stdio_mode() {
        let cmd = parse_cli(&[
            "mcpway",
            "connect",
            "--stdio-cmd",
            "node server.js",
            "--stdio-arg",
            "--debug",
            "--stdio-env",
            "API_KEY=abc123",
            "--save-wrapper",
            "./wrapper-out",
        ])
        .expect("connect stdio parse failed");

        match cmd {
            CliCommand::Connect(cfg) => {
                assert_eq!(cfg.endpoint, None);
                assert_eq!(cfg.server, None);
                assert_eq!(cfg.stdio_cmd, Some("node server.js".to_string()));
                assert_eq!(cfg.stdio_args, vec!["--debug".to_string()]);
                assert_eq!(cfg.stdio_env.get("API_KEY"), Some(&"abc123".to_string()));
                assert_eq!(cfg.save_wrapper_dir, Some(PathBuf::from("./wrapper-out")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_connect_subcommand_oauth_options() {
        let cmd = parse_cli(&[
            "mcpway",
            "connect",
            "https://example.com/mcp",
            "--oauth-issuer",
            "https://issuer.example.com",
            "--oauth-client-id",
            "client-123",
            "--oauth-scope",
            "mcp.read",
            "--oauth-flow",
            "auth-code",
            "--oauth-login",
        ])
        .expect("connect oauth parse failed");

        match cmd {
            CliCommand::Connect(cfg) => {
                let oauth = cfg.oauth.expect("oauth config missing");
                assert_eq!(oauth.issuer, "https://issuer.example.com");
                assert_eq!(oauth.client_id, "client-123");
                assert_eq!(oauth.scopes, vec!["mcp.read".to_string()]);
                assert_eq!(oauth.flow, OAuthFlow::AuthCode);
                assert!(oauth.login);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_discover_subcommand_defaults() {
        let cmd = parse_cli(&["mcpway", "discover"]).expect("discover parse failed");
        match cmd {
            CliCommand::Discover(cfg) => {
                assert_eq!(cfg.from, ImportSource::Auto);
                assert_eq!(cfg.project_root, None);
                assert!(!cfg.print_json);
                assert!(!cfg.strict_conflicts);
                assert_eq!(cfg.search, None);
                assert_eq!(cfg.transport, None);
                assert_eq!(cfg.scope, None);
                assert!(!cfg.enabled_only);
                assert_eq!(cfg.sort_by, DiscoverSortBy::Name);
                assert_eq!(cfg.order, SortOrder::Asc);
                assert_eq!(cfg.offset, 0);
                assert_eq!(cfg.limit, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_discover_subcommand_nodecode_source() {
        let cmd = parse_cli(&["mcpway", "discover", "--from", "nodecode"])
            .expect("discover parse failed");
        match cmd {
            CliCommand::Discover(cfg) => {
                assert_eq!(cfg.from, ImportSource::Nodecode);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_discover_subcommand_filters() {
        let cmd = parse_cli(&[
            "mcpway",
            "discover",
            "--search",
            "github",
            "--transport",
            "streamable-http",
            "--scope",
            "project",
            "--enabled-only",
            "--sort",
            "origin-path",
            "--order",
            "desc",
            "--offset",
            "10",
            "--limit",
            "25",
        ])
        .expect("discover filters should parse");

        match cmd {
            CliCommand::Discover(cfg) => {
                assert_eq!(cfg.search.as_deref(), Some("github"));
                assert_eq!(cfg.transport, Some(DiscoverTransport::StreamableHttp));
                assert_eq!(cfg.scope, Some(DiscoverScope::Project));
                assert!(cfg.enabled_only);
                assert_eq!(cfg.sort_by, DiscoverSortBy::OriginPath);
                assert_eq!(cfg.order, SortOrder::Desc);
                assert_eq!(cfg.offset, 10);
                assert_eq!(cfg.limit, Some(25));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_import_subcommand_options() {
        let cmd = parse_cli(&[
            "mcpway",
            "import",
            "--from",
            "vscode",
            "--project-root",
            "./project",
            "--json",
            "--strict-conflicts",
            "--registry",
            "./registry.json",
            "--save-profiles",
            "./profiles",
            "--bundle-mcpway",
            "--compile-wrapper",
        ])
        .expect("import parse failed");

        match cmd {
            CliCommand::Import(cfg) => {
                assert_eq!(cfg.from, ImportSource::Vscode);
                assert_eq!(cfg.project_root, Some(PathBuf::from("./project")));
                assert!(cfg.print_json);
                assert!(cfg.strict_conflicts);
                assert_eq!(cfg.registry_path, Some(PathBuf::from("./registry.json")));
                assert_eq!(cfg.save_profiles_dir, Some(PathBuf::from("./profiles")));
                assert!(cfg.bundle_mcpway);
                assert!(cfg.compile_wrapper);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_import_subcommand_nodecode_source() {
        let cmd =
            parse_cli(&["mcpway", "import", "--from", "nodecode"]).expect("import parse failed");
        match cmd {
            CliCommand::Import(cfg) => {
                assert_eq!(cfg.from, ImportSource::Nodecode);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_logs_tail_subcommand_defaults() {
        let cmd = parse_cli(&["mcpway", "logs", "tail"]).expect("logs tail parse failed");
        match cmd {
            CliCommand::Logs(LogsConfig::Tail(cfg)) => {
                assert_eq!(cfg.file, None);
                assert!(cfg.follow);
                assert_eq!(cfg.lines, 200);
                assert_eq!(cfg.level, None);
                assert_eq!(cfg.transport, None);
                assert!(!cfg.json);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn cli_help_contains_usage_and_transport_flags() {
        let help = cli_help_text();
        assert!(help.contains("Usage: mcpway [OPTIONS]"));
        assert!(help.contains("--stdio <CMD>"));
        assert!(help.contains("--sse <URL>"));
        assert!(help.contains("--streamable-http <URL>"));
    }

    #[test]
    fn no_args_banner_contains_examples_and_help() {
        let banner = no_args_banner_text_with_style(false);
        assert!(banner.contains("MCPway CLI"));
        assert!(banner.contains("Generator Subcommands"));
        assert!(banner.contains("Ad-hoc Connect"));
        assert!(banner.contains("Zero-Config Discovery"));
        assert!(banner.contains("Quick Start"));
        assert!(banner.contains("Full Options"));
        assert!(banner.contains("mcpway --stdio"));
        assert!(banner.contains("Usage: mcpway [OPTIONS]"));
    }
}
