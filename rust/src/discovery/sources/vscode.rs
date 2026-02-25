use std::path::{Path, PathBuf};

use crate::discovery::{DiscoveredServer, DiscoveryIssue, DiscoveryScope, SourceKind};

use super::{infer_transport, json_string_array, json_string_map, load_json, source_issue};

pub fn discover(
    project_root: &Path,
    home_dir: Option<&Path>,
) -> (Vec<DiscoveredServer>, Vec<DiscoveryIssue>) {
    let mut servers = Vec::new();
    let mut issues = Vec::new();

    let project_path = project_root.join(".vscode").join("mcp.json");
    collect_from_path(
        &project_path,
        DiscoveryScope::Project,
        &mut servers,
        &mut issues,
    );

    if let Some(home) = home_dir {
        for candidate in global_candidates(home) {
            collect_from_path(
                &candidate,
                DiscoveryScope::Global,
                &mut servers,
                &mut issues,
            );
        }
    }

    (servers, issues)
}

fn global_candidates(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".config")
            .join("Code")
            .join("User")
            .join("mcp.json"),
        home.join(".config")
            .join("Code - Insiders")
            .join("User")
            .join("mcp.json"),
        home.join(".vscode").join("mcp.json"),
    ]
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
            issues.push(source_issue(SourceKind::VsCode, path, err));
            return;
        }
    };

    let servers_obj = root
        .get("servers")
        .and_then(|v| v.as_object())
        .or_else(|| root.get("mcpServers").and_then(|v| v.as_object()));

    let Some(servers_obj) = servers_obj else {
        issues.push(source_issue(
            SourceKind::VsCode,
            path,
            "Missing or invalid 'servers' object",
        ));
        return;
    };

    for (name, server) in servers_obj {
        let Some(server_obj) = server.as_object() else {
            issues.push(source_issue(
                SourceKind::VsCode,
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
            .get("url")
            .and_then(|v| v.as_str())
            .or_else(|| server_obj.get("serverUrl").and_then(|v| v.as_str()))
            .map(|v| v.to_string());
        let explicit_type = server_obj.get("type").and_then(|v| v.as_str());

        let transport = match infer_transport(command.as_deref(), url.as_deref(), explicit_type) {
            Some(transport) => transport,
            None => {
                issues.push(source_issue(
                    SourceKind::VsCode,
                    path,
                    format!("Server '{name}' is missing transport fields (command/url/type)"),
                ));
                continue;
            }
        };

        out.push(DiscoveredServer {
            name: name.to_string(),
            source: SourceKind::VsCode,
            scope,
            origin_path: path.to_string_lossy().to_string(),
            transport,
            command,
            args,
            url,
            headers,
            env,
            enabled: true,
            raw_format: "vscode-mcp-json".to_string(),
        });
    }
}
