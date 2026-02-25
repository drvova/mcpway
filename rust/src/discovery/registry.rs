use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::ConnectProtocol;
use crate::discovery::{DiscoveredServer, DiscoveredTransport};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedRegistry {
    pub schema_version: String,
    pub generated_at_utc: String,
    pub servers: Vec<DiscoveredServer>,
}

#[derive(Debug, Clone)]
pub enum ResolvedImportedServer {
    Remote {
        name: String,
        endpoint: String,
        protocol: ConnectProtocol,
        headers: HashMap<String, String>,
    },
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
}

pub fn default_registry_path() -> PathBuf {
    if let Some(home) = crate::discovery::user_home_dir() {
        return home.join(".mcpway").join("imported-mcp-registry.json");
    }

    PathBuf::from(".mcpway/imported-mcp-registry.json")
}

pub fn write_registry(
    path: &Path,
    servers: &[DiscoveredServer],
) -> Result<ImportedRegistry, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid registry path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;

    let registry = ImportedRegistry {
        schema_version: "1".to_string(),
        generated_at_utc: unix_timestamp_utc_string(),
        servers: servers.to_vec(),
    };

    let json = serde_json::to_string_pretty(&registry)
        .map_err(|err| format!("Failed to serialize registry: {err}"))?;
    std::fs::write(path, json)
        .map_err(|err| format!("Failed to write registry {}: {err}", path.display()))?;

    Ok(registry)
}

pub fn read_registry(path: &Path) -> Result<ImportedRegistry, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read registry {}: {err}", path.display()))?;
    serde_json::from_str::<ImportedRegistry>(&body)
        .map_err(|err| format!("Invalid registry JSON in {}: {err}", path.display()))
}

pub fn resolve_server(
    name: &str,
    registry_path: Option<&Path>,
) -> Result<ResolvedImportedServer, String> {
    let path = registry_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_registry_path);
    let registry = read_registry(&path)?;

    let server = registry
        .servers
        .into_iter()
        .find(|server| server.name == name)
        .ok_or_else(|| format!("Server '{name}' not found in registry {}", path.display()))?;

    match server.transport {
        DiscoveredTransport::Stdio => {
            let command = server
                .command
                .as_ref()
                .ok_or_else(|| format!("Server '{name}' is stdio but missing command"))?;
            let args = server.args;
            let env = server.env.into_iter().collect::<HashMap<_, _>>();

            Ok(ResolvedImportedServer::Stdio {
                name: server.name,
                command: command.to_string(),
                args,
                env,
            })
        }
        DiscoveredTransport::Sse
        | DiscoveredTransport::Ws
        | DiscoveredTransport::StreamableHttp => {
            let endpoint = server
                .url
                .as_ref()
                .ok_or_else(|| format!("Server '{name}' is remote but missing URL"))?;
            let headers = server.headers.into_iter().collect::<HashMap<_, _>>();

            let protocol = match server.transport {
                DiscoveredTransport::Sse => ConnectProtocol::Sse,
                DiscoveredTransport::Ws => ConnectProtocol::Ws,
                DiscoveredTransport::StreamableHttp => ConnectProtocol::StreamableHttp,
                DiscoveredTransport::Stdio => unreachable!(),
            };

            Ok(ResolvedImportedServer::Remote {
                name: server.name,
                endpoint: endpoint.to_string(),
                protocol,
                headers,
            })
        }
    }
}

fn unix_timestamp_utc_string() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("{}", duration.as_secs()),
        Err(_) => "0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{DiscoveryScope, SourceKind};
    use std::collections::BTreeMap;

    #[test]
    fn resolve_server_keeps_literal_placeholder_values() {
        let path = std::env::temp_dir().join(format!(
            "mcpway-registry-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos()
        ));
        let server = DiscoveredServer {
            name: "demo".to_string(),
            source: SourceKind::Cursor,
            scope: DiscoveryScope::Project,
            origin_path: "/tmp/x".to_string(),
            transport: DiscoveredTransport::StreamableHttp,
            command: None,
            args: Vec::new(),
            url: Some("https://example.com/mcp".to_string()),
            headers: BTreeMap::from([(
                "Authorization".to_string(),
                "Bearer ${DEMO_TOKEN}".to_string(),
            )]),
            env: BTreeMap::new(),
            enabled: true,
            raw_format: "test".to_string(),
        };

        write_registry(&path, &[server]).expect("write registry");
        let resolved = resolve_server("demo", Some(path.as_path())).expect("resolve server");

        match resolved {
            ResolvedImportedServer::Remote { headers, .. } => {
                assert_eq!(
                    headers.get("Authorization"),
                    Some(&"Bearer ${DEMO_TOKEN}".to_string())
                );
            }
            other => panic!("unexpected server variant: {other:?}"),
        }

        let _ = std::fs::remove_file(path);
    }
}
