use std::path::Path;

use crate::discovery::{DiscoveredServer, DiscoveryIssue, DiscoveryScope, SourceKind};

use super::{infer_transport, json_string_array, json_string_map, load_json, source_issue};

pub fn discover(
    project_root: &Path,
    home_dir: Option<&Path>,
) -> (Vec<DiscoveredServer>, Vec<DiscoveryIssue>) {
    let mut servers = Vec::new();
    let mut issues = Vec::new();

    let project_path = project_root.join(".nodecode").join("config.json");
    collect_from_path(
        &project_path,
        DiscoveryScope::Project,
        &mut servers,
        &mut issues,
    );

    if let Some(home) = home_dir {
        let global_path = home.join(".nodecode").join("config.json");
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

    let root = match load_json(path) {
        Ok(root) => root,
        Err(err) => {
            issues.push(source_issue(SourceKind::Nodecode, path, err));
            return;
        }
    };

    let servers_obj = root
        .get("mcp")
        .and_then(|v| v.as_object())
        .and_then(|mcp| mcp.get("servers"))
        .and_then(|v| v.as_object());

    let Some(servers_obj) = servers_obj else {
        issues.push(source_issue(
            SourceKind::Nodecode,
            path,
            "Missing or invalid 'mcp.servers' object",
        ));
        return;
    };

    for (name, server) in servers_obj {
        let Some(server_obj) = server.as_object() else {
            issues.push(source_issue(
                SourceKind::Nodecode,
                path,
                format!("mcp.servers.{name} must be an object"),
            ));
            continue;
        };

        if server_obj
            .get("enabled")
            .and_then(|v| v.as_bool())
            .is_some_and(|enabled| !enabled)
        {
            continue;
        }

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
            .map(|v| v.to_string());
        let explicit_type = server_obj.get("type").and_then(|v| v.as_str());

        let transport = match infer_transport(command.as_deref(), url.as_deref(), explicit_type) {
            Some(transport) => transport,
            None => {
                issues.push(source_issue(
                    SourceKind::Nodecode,
                    path,
                    format!("mcp.servers.{name} is missing transport fields (command/url/type)"),
                ));
                continue;
            }
        };

        out.push(DiscoveredServer {
            name: name.to_string(),
            source: SourceKind::Nodecode,
            scope,
            origin_path: path.to_string_lossy().to_string(),
            transport,
            command,
            args,
            url,
            headers,
            env,
            enabled: true,
            raw_format: "nodecode-config-json".to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock drift")
            .as_nanos();
        std::env::temp_dir().join(format!("mcpway-nodecode-{name}-{suffix}"))
    }

    #[test]
    fn discovers_project_nodecode_servers() {
        let project = unique_temp_dir("project");
        let nodecode_dir = project.join(".nodecode");
        std::fs::create_dir_all(&nodecode_dir).expect("create project .nodecode");
        std::fs::write(
            nodecode_dir.join("config.json"),
            r#"{
  "mcp": {
    "servers": {
      "demo": {
        "command": "node",
        "args": ["server.js"],
        "env": {"API_KEY": "abc"},
        "headers": {"X-Test": "1"}
      }
    }
  }
}"#,
        )
        .expect("write config");

        let (servers, issues) = discover(&project, None);
        assert!(issues.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "demo");
        assert_eq!(servers[0].source, SourceKind::Nodecode);
        assert_eq!(servers[0].scope, DiscoveryScope::Project);
        assert_eq!(
            servers[0].transport,
            crate::discovery::DiscoveredTransport::Stdio
        );

        let _ = std::fs::remove_dir_all(project);
    }

    #[test]
    fn discovers_global_nodecode_servers() {
        let project = unique_temp_dir("project-empty");
        let home = unique_temp_dir("home");
        let nodecode_dir = home.join(".nodecode");
        std::fs::create_dir_all(&nodecode_dir).expect("create home .nodecode");
        std::fs::write(
            nodecode_dir.join("config.json"),
            r#"{
  "mcp": {
    "servers": {
      "remote": {
        "url": "https://example.com/mcp"
      }
    }
  }
}"#,
        )
        .expect("write config");

        let (servers, issues) = discover(&project, Some(&home));
        assert!(issues.is_empty());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "remote");
        assert_eq!(servers[0].scope, DiscoveryScope::Global);
        assert_eq!(
            servers[0].transport,
            crate::discovery::DiscoveredTransport::StreamableHttp
        );

        let _ = std::fs::remove_dir_all(project);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn emits_issue_when_mcp_servers_missing() {
        let project = unique_temp_dir("missing");
        let nodecode_dir = project.join(".nodecode");
        std::fs::create_dir_all(&nodecode_dir).expect("create project .nodecode");
        std::fs::write(nodecode_dir.join("config.json"), r#"{"mcp": {}}"#).expect("write config");

        let (servers, issues) = discover(&project, None);
        assert!(servers.is_empty());
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("mcp.servers"));

        let _ = std::fs::remove_dir_all(project);
    }
}
