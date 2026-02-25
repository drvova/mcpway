use std::collections::BTreeMap;
use std::path::Path;

use crate::discovery::{DiscoveredServer, DiscoveryIssue, DiscoveryScope, SourceKind};

use super::{infer_transport, load_toml, source_issue};

pub fn discover(
    project_root: &Path,
    home_dir: Option<&Path>,
) -> (Vec<DiscoveredServer>, Vec<DiscoveryIssue>) {
    let mut servers = Vec::new();
    let mut issues = Vec::new();

    let project_path = project_root.join(".codex").join("config.toml");
    collect_from_path(
        &project_path,
        DiscoveryScope::Project,
        &mut servers,
        &mut issues,
    );

    if let Some(home) = home_dir {
        let global_path = home.join(".codex").join("config.toml");
        collect_from_path(
            &global_path,
            DiscoveryScope::Global,
            &mut servers,
            &mut issues,
        );
    }

    (servers, issues)
}

fn collect_from_path(
    path: &Path,
    scope: DiscoveryScope,
    out: &mut Vec<DiscoveredServer>,
    issues: &mut Vec<DiscoveryIssue>,
) {
    if !path.exists() {
        return;
    }

    let root = match load_toml(path) {
        Ok(root) => root,
        Err(err) => {
            issues.push(source_issue(SourceKind::Codex, path, err));
            return;
        }
    };

    let Some(mcp_servers) = root.get("mcp_servers").and_then(|v| v.as_table()) else {
        return;
    };

    for (name, raw_server) in mcp_servers {
        let Some(server) = raw_server.as_table() else {
            issues.push(source_issue(
                SourceKind::Codex,
                path,
                format!("mcp_servers.{name} must be a table"),
            ));
            continue;
        };

        if server
            .get("enabled")
            .and_then(|v| v.as_bool())
            .is_some_and(|enabled| !enabled)
        {
            continue;
        }

        let command = server
            .get("command")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let args = server
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let env = toml_string_map(server.get("env"));

        let mut headers = toml_string_map(server.get("http_headers"));

        if let Some(env_headers) = server.get("env_http_headers").and_then(|v| v.as_table()) {
            for (header, env_var) in env_headers {
                if let Some(key) = env_var.as_str() {
                    headers.insert(header.to_string(), format!("${{{key}}}"));
                }
            }
        }

        if let Some(token_var) = server.get("bearer_token_env_var").and_then(|v| v.as_str()) {
            headers
                .entry("Authorization".to_string())
                .or_insert_with(|| format!("Bearer ${{{token_var}}}"));
        }

        let url = server
            .get("url")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        let transport = match infer_transport(command.as_deref(), url.as_deref(), None) {
            Some(transport) => transport,
            None => {
                issues.push(source_issue(
                    SourceKind::Codex,
                    path,
                    format!("mcp_servers.{name} is missing 'command' or 'url'"),
                ));
                continue;
            }
        };

        out.push(DiscoveredServer {
            name: name.to_string(),
            source: SourceKind::Codex,
            scope,
            origin_path: path.to_string_lossy().to_string(),
            transport,
            command,
            args,
            url,
            headers,
            env,
            enabled: true,
            raw_format: "codex-config-toml".to_string(),
        });
    }
}

fn toml_string_map(value: Option<&toml::Value>) -> BTreeMap<String, String> {
    let Some(table) = value.and_then(|v| v.as_table()) else {
        return BTreeMap::new();
    };

    let mut out = BTreeMap::new();
    for (key, value) in table {
        if let Some(v) = value.as_str() {
            out.insert(key.to_string(), v.to_string());
        }
    }
    out
}
