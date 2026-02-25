use std::path::Path;

use crate::discovery::{DiscoveredServer, DiscoveryIssue, DiscoveryScope, SourceKind};

use super::{infer_transport, json_string_array, json_string_map, load_json, source_issue};

pub fn discover(
    _project_root: &Path,
    home_dir: Option<&Path>,
) -> (Vec<DiscoveredServer>, Vec<DiscoveryIssue>) {
    let mut servers = Vec::new();
    let mut issues = Vec::new();

    let Some(home) = home_dir else {
        return (servers, issues);
    };

    let primary_global_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    collect_from_path(
        &primary_global_path,
        DiscoveryScope::Global,
        &mut servers,
        &mut issues,
    );

    let legacy_global_path = home.join(".codeium").join("mcp_config.json");
    collect_from_path(
        &legacy_global_path,
        DiscoveryScope::Global,
        &mut servers,
        &mut issues,
    );

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

    let root = match load_json(path) {
        Ok(root) => root,
        Err(err) => {
            issues.push(source_issue(SourceKind::Windsurf, path, err));
            return;
        }
    };

    let Some(servers_obj) = root.get("mcpServers").and_then(|v| v.as_object()) else {
        issues.push(source_issue(
            SourceKind::Windsurf,
            path,
            "Missing or invalid 'mcpServers' object",
        ));
        return;
    };

    for (name, server) in servers_obj {
        let Some(server_obj) = server.as_object() else {
            issues.push(source_issue(
                SourceKind::Windsurf,
                path,
                format!("Server '{name}' must be an object"),
            ));
            continue;
        };

        let command = server_obj
            .get("command")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let args = json_string_array(server_obj.get("args"));
        let env = json_string_map(server_obj.get("env"));
        let headers = json_string_map(server_obj.get("headers"));
        let url = server_obj
            .get("serverUrl")
            .and_then(|v| v.as_str())
            .or_else(|| server_obj.get("url").and_then(|v| v.as_str()))
            .map(|v| v.to_string());
        let explicit_type = server_obj.get("type").and_then(|v| v.as_str());

        let transport = match infer_transport(command.as_deref(), url.as_deref(), explicit_type) {
            Some(transport) => transport,
            None => {
                issues.push(source_issue(
                    SourceKind::Windsurf,
                    path,
                    format!("Server '{name}' is missing transport fields (command/url/type)"),
                ));
                continue;
            }
        };

        out.push(DiscoveredServer {
            name: name.to_string(),
            source: SourceKind::Windsurf,
            scope,
            origin_path: path.to_string_lossy().to_string(),
            transport,
            command,
            args,
            url,
            headers,
            env,
            enabled: true,
            raw_format: "windsurf-mcp-config".to_string(),
        });
    }
}
