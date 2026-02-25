use std::path::Path;

use crate::discovery::{DiscoveredServer, DiscoveryIssue, DiscoveryScope, SourceKind};

use super::{infer_transport, json_string_map, load_json_or_jsonc, source_issue};

pub fn discover(
    project_root: &Path,
    home_dir: Option<&Path>,
) -> (Vec<DiscoveredServer>, Vec<DiscoveryIssue>) {
    let mut servers = Vec::new();
    let mut issues = Vec::new();

    let project_path = project_root.join("opencode.json");
    collect_from_path(
        &project_path,
        DiscoveryScope::Project,
        &mut servers,
        &mut issues,
    );

    let project_path_jsonc = project_root.join("opencode.jsonc");
    collect_from_path(
        &project_path_jsonc,
        DiscoveryScope::Project,
        &mut servers,
        &mut issues,
    );

    if let Some(home) = home_dir {
        let global_json = home.join(".config").join("opencode").join("opencode.json");
        collect_from_path(
            &global_json,
            DiscoveryScope::Global,
            &mut servers,
            &mut issues,
        );

        let global_jsonc = home.join(".config").join("opencode").join("opencode.jsonc");
        collect_from_path(
            &global_jsonc,
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

    let root = match load_json_or_jsonc(path) {
        Ok(root) => root,
        Err(err) => {
            issues.push(source_issue(SourceKind::OpenCode, path, err));
            return;
        }
    };

    let Some(mcp_obj) = root.get("mcp").and_then(|v| v.as_object()) else {
        return;
    };

    for (name, raw_server) in mcp_obj {
        let Some(server) = raw_server.as_object() else {
            issues.push(source_issue(
                SourceKind::OpenCode,
                path,
                format!("mcp.{name} must be an object"),
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

        let explicit_type = server.get("type").and_then(|v| v.as_str());
        let mut command = server
            .get("command")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let mut args = Vec::new();

        if command.is_none() {
            if let Some(arr) = server.get("command").and_then(|v| v.as_array()) {
                if let Some(first) = arr.first().and_then(|v| v.as_str()) {
                    command = Some(first.to_string());
                    args.extend(
                        arr.iter()
                            .skip(1)
                            .filter_map(|v| v.as_str())
                            .map(ToString::to_string),
                    );
                }
            }
        }

        if args.is_empty() {
            args.extend(
                server
                    .get("args")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .map(ToString::to_string),
            );
        }

        let env = json_string_map(server.get("environment").or_else(|| server.get("env")));
        let headers = json_string_map(server.get("headers"));
        let url = server
            .get("url")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        let transport = match infer_transport(command.as_deref(), url.as_deref(), explicit_type) {
            Some(transport) => transport,
            None => {
                issues.push(source_issue(
                    SourceKind::OpenCode,
                    path,
                    format!("mcp.{name} is missing transport fields (command/url/type)"),
                ));
                continue;
            }
        };

        out.push(DiscoveredServer {
            name: name.to_string(),
            source: SourceKind::OpenCode,
            scope,
            origin_path: path.to_string_lossy().to_string(),
            transport,
            command,
            args,
            url,
            headers,
            env,
            enabled: true,
            raw_format: "opencode-config".to_string(),
        });
    }
}
