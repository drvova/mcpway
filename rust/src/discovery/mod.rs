pub mod registry;
pub mod sources;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Cursor,
    Claude,
    Codex,
    Windsurf,
    OpenCode,
    Nodecode,
    VsCode,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Windsurf => "windsurf",
            Self::OpenCode => "opencode",
            Self::Nodecode => "nodecode",
            Self::VsCode => "vscode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryScope {
    Project,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveredTransport {
    Stdio,
    Sse,
    Ws,
    StreamableHttp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredServer {
    pub name: String,
    pub source: SourceKind,
    pub scope: DiscoveryScope,
    pub origin_path: String,
    pub transport: DiscoveredTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub enabled: bool,
    pub raw_format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryIssueLevel {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryIssue {
    pub level: DiscoveryIssueLevel,
    pub source: SourceKind,
    pub origin_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConflict {
    pub name: String,
    pub kept_source: SourceKind,
    pub kept_scope: DiscoveryScope,
    pub kept_origin_path: String,
    pub dropped_source: SourceKind,
    pub dropped_scope: DiscoveryScope,
    pub dropped_origin_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub project_root: String,
    pub servers: Vec<DiscoveredServer>,
    pub conflicts: Vec<DiscoveryConflict>,
    pub issues: Vec<DiscoveryIssue>,
}

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    pub from: Option<SourceKind>,
    pub project_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverySortField {
    #[default]
    Name,
    Source,
    Scope,
    Transport,
    OriginPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiscoverySortOrder {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoverySearchOptions {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub transport: Option<DiscoveredTransport>,
    #[serde(default)]
    pub scope: Option<DiscoveryScope>,
    #[serde(default)]
    pub source: Option<SourceKind>,
    #[serde(default)]
    pub enabled_only: bool,
    #[serde(default)]
    pub sort_by: DiscoverySortField,
    #[serde(default)]
    pub sort_order: DiscoverySortOrder,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub const SOURCE_PRIORITY: [SourceKind; 7] = [
    SourceKind::Cursor,
    SourceKind::Claude,
    SourceKind::Codex,
    SourceKind::Windsurf,
    SourceKind::OpenCode,
    SourceKind::Nodecode,
    SourceKind::VsCode,
];

pub fn discover(options: &DiscoverOptions) -> Result<DiscoveryReport, String> {
    let project_root = resolve_project_root(options.project_root.as_deref())?;
    let home_dir = user_home_dir();
    let selected_sources = if let Some(source) = options.from {
        vec![source]
    } else {
        SOURCE_PRIORITY.to_vec()
    };

    let mut issues = Vec::new();
    let mut by_source: HashMap<SourceKind, Vec<DiscoveredServer>> = HashMap::new();

    for source in &selected_sources {
        let (servers, source_issues) = match source {
            SourceKind::Cursor => sources::cursor::discover(&project_root, home_dir.as_deref()),
            SourceKind::Claude => sources::claude::discover(&project_root, home_dir.as_deref()),
            SourceKind::Codex => sources::codex::discover(&project_root, home_dir.as_deref()),
            SourceKind::Windsurf => sources::windsurf::discover(&project_root, home_dir.as_deref()),
            SourceKind::OpenCode => sources::opencode::discover(&project_root, home_dir.as_deref()),
            SourceKind::Nodecode => sources::nodecode::discover(&project_root, home_dir.as_deref()),
            SourceKind::VsCode => sources::vscode::discover(&project_root, home_dir.as_deref()),
        };

        issues.extend(source_issues);
        by_source.insert(*source, collapse_source_scope_precedence(servers));
    }

    let mut conflicts = Vec::new();
    let mut by_name: BTreeMap<String, DiscoveredServer> = BTreeMap::new();

    for source in SOURCE_PRIORITY
        .iter()
        .copied()
        .filter(|source| selected_sources.contains(source))
    {
        if let Some(mut source_servers) = by_source.remove(&source) {
            source_servers.sort_by(|a, b| a.name.cmp(&b.name));
            for server in source_servers {
                if let Some(existing) = by_name.get(&server.name) {
                    conflicts.push(DiscoveryConflict {
                        name: server.name.clone(),
                        kept_source: existing.source,
                        kept_scope: existing.scope,
                        kept_origin_path: existing.origin_path.clone(),
                        dropped_source: server.source,
                        dropped_scope: server.scope,
                        dropped_origin_path: server.origin_path.clone(),
                    });
                    continue;
                }
                by_name.insert(server.name.clone(), server);
            }
        }
    }

    let servers = by_name.into_values().collect::<Vec<_>>();

    Ok(DiscoveryReport {
        project_root: project_root.to_string_lossy().to_string(),
        servers,
        conflicts,
        issues,
    })
}

pub fn apply_search(report: &DiscoveryReport, options: &DiscoverySearchOptions) -> DiscoveryReport {
    let query = options
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());

    let mut servers = report
        .servers
        .iter()
        .filter(|server| {
            if let Some(transport) = options.transport {
                if server.transport != transport {
                    return false;
                }
            }
            if let Some(scope) = options.scope {
                if server.scope != scope {
                    return false;
                }
            }
            if let Some(source) = options.source {
                if server.source != source {
                    return false;
                }
            }
            if options.enabled_only && !server.enabled {
                return false;
            }
            if let Some(query) = query.as_deref() {
                if !server_matches_query(server, query) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect::<Vec<_>>();

    servers.sort_by(|left, right| compare_servers(left, right, options.sort_by));
    if options.sort_order == DiscoverySortOrder::Desc {
        servers.reverse();
    }

    if options.offset > 0 {
        servers = servers.into_iter().skip(options.offset).collect();
    }
    if let Some(limit) = options.limit {
        servers.truncate(limit);
    }

    DiscoveryReport {
        project_root: report.project_root.clone(),
        servers,
        conflicts: report.conflicts.clone(),
        issues: report.issues.clone(),
    }
}

fn collapse_source_scope_precedence(servers: Vec<DiscoveredServer>) -> Vec<DiscoveredServer> {
    let mut sorted = servers;
    sorted.sort_by(|a, b| {
        let primary = a.name.cmp(&b.name);
        if primary != std::cmp::Ordering::Equal {
            return primary;
        }
        scope_priority(a.scope).cmp(&scope_priority(b.scope))
    });

    let mut out = BTreeMap::new();
    for server in sorted {
        out.entry(server.name.clone()).or_insert(server);
    }

    out.into_values().collect()
}

fn scope_priority(scope: DiscoveryScope) -> u8 {
    match scope {
        DiscoveryScope::Project => 0,
        DiscoveryScope::Global => 1,
    }
}

fn server_matches_query(server: &DiscoveredServer, query: &str) -> bool {
    let command = server
        .command
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let url = server
        .url
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let source = server.source.as_str().to_ascii_lowercase();
    let transport = match server.transport {
        DiscoveredTransport::Stdio => "stdio",
        DiscoveredTransport::Sse => "sse",
        DiscoveredTransport::Ws => "ws",
        DiscoveredTransport::StreamableHttp => "streamable-http",
    };
    let scope = match server.scope {
        DiscoveryScope::Project => "project",
        DiscoveryScope::Global => "global",
    };

    server.name.to_ascii_lowercase().contains(query)
        || source.contains(query)
        || scope.contains(query)
        || transport.contains(query)
        || server.origin_path.to_ascii_lowercase().contains(query)
        || server.raw_format.to_ascii_lowercase().contains(query)
        || command.contains(query)
        || url.contains(query)
}

fn compare_servers(
    left: &DiscoveredServer,
    right: &DiscoveredServer,
    sort_by: DiscoverySortField,
) -> Ordering {
    let primary = match sort_by {
        DiscoverySortField::Name => left.name.cmp(&right.name),
        DiscoverySortField::Source => left.source.as_str().cmp(right.source.as_str()),
        DiscoverySortField::Scope => scope_priority(left.scope).cmp(&scope_priority(right.scope)),
        DiscoverySortField::Transport => {
            transport_priority(left.transport).cmp(&transport_priority(right.transport))
        }
        DiscoverySortField::OriginPath => left.origin_path.cmp(&right.origin_path),
    };

    if primary != Ordering::Equal {
        return primary;
    }

    left.name
        .cmp(&right.name)
        .then_with(|| left.source.as_str().cmp(right.source.as_str()))
        .then_with(|| left.origin_path.cmp(&right.origin_path))
}

fn transport_priority(transport: DiscoveredTransport) -> u8 {
    match transport {
        DiscoveredTransport::Stdio => 0,
        DiscoveredTransport::Sse => 1,
        DiscoveredTransport::Ws => 2,
        DiscoveredTransport::StreamableHttp => 3,
    }
}

pub fn resolve_project_root(input: Option<&Path>) -> Result<PathBuf, String> {
    let start = if let Some(path) = input {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|err| format!("Failed to read cwd: {err}"))?
                .join(path)
        }
    } else {
        std::env::current_dir().map_err(|err| format!("Failed to read cwd: {err}"))?
    };

    Ok(find_nearest_git_root(&start).unwrap_or(start))
}

fn find_nearest_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().map(Path::to_path_buf)
    } else {
        Some(start.to_path_buf())
    }?;

    loop {
        let git_path = current.join(".git");
        if git_path.exists() {
            return Some(current);
        }

        if !current.pop() {
            break;
        }
    }

    None
}

pub fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_precedence_prefers_project() {
        let global = DiscoveredServer {
            name: "shared".to_string(),
            source: SourceKind::Cursor,
            scope: DiscoveryScope::Global,
            origin_path: "/tmp/global".to_string(),
            transport: DiscoveredTransport::StreamableHttp,
            command: None,
            args: Vec::new(),
            url: Some("https://example.com/mcp".to_string()),
            headers: BTreeMap::new(),
            env: BTreeMap::new(),
            enabled: true,
            raw_format: "test".to_string(),
        };

        let mut project = global.clone();
        project.scope = DiscoveryScope::Project;
        project.origin_path = "/tmp/project".to_string();

        let collapsed = collapse_source_scope_precedence(vec![global, project]);
        assert_eq!(collapsed.len(), 1);
        assert_eq!(collapsed[0].scope, DiscoveryScope::Project);
        assert_eq!(collapsed[0].origin_path, "/tmp/project");
    }

    #[test]
    fn apply_search_filters_sorts_and_pages() {
        let report = DiscoveryReport {
            project_root: "/tmp/project".to_string(),
            servers: vec![
                DiscoveredServer {
                    name: "alpha".to_string(),
                    source: SourceKind::OpenCode,
                    scope: DiscoveryScope::Project,
                    origin_path: "/tmp/project/a".to_string(),
                    transport: DiscoveredTransport::StreamableHttp,
                    command: None,
                    args: Vec::new(),
                    url: Some("https://a.example/mcp".to_string()),
                    headers: BTreeMap::new(),
                    env: BTreeMap::new(),
                    enabled: true,
                    raw_format: "json".to_string(),
                },
                DiscoveredServer {
                    name: "beta".to_string(),
                    source: SourceKind::OpenCode,
                    scope: DiscoveryScope::Global,
                    origin_path: "/tmp/global/b".to_string(),
                    transport: DiscoveredTransport::Stdio,
                    command: Some("node".to_string()),
                    args: Vec::new(),
                    url: None,
                    headers: BTreeMap::new(),
                    env: BTreeMap::new(),
                    enabled: false,
                    raw_format: "json".to_string(),
                },
                DiscoveredServer {
                    name: "gamma".to_string(),
                    source: SourceKind::Cursor,
                    scope: DiscoveryScope::Project,
                    origin_path: "/tmp/project/g".to_string(),
                    transport: DiscoveredTransport::StreamableHttp,
                    command: None,
                    args: Vec::new(),
                    url: Some("https://g.example/mcp".to_string()),
                    headers: BTreeMap::new(),
                    env: BTreeMap::new(),
                    enabled: true,
                    raw_format: "json".to_string(),
                },
            ],
            conflicts: Vec::new(),
            issues: Vec::new(),
        };

        let options = DiscoverySearchOptions {
            query: Some("example".to_string()),
            transport: Some(DiscoveredTransport::StreamableHttp),
            scope: Some(DiscoveryScope::Project),
            source: None,
            enabled_only: true,
            sort_by: DiscoverySortField::Name,
            sort_order: DiscoverySortOrder::Desc,
            offset: 1,
            limit: Some(1),
        };
        let filtered = apply_search(&report, &options);
        assert_eq!(filtered.servers.len(), 1);
        assert_eq!(filtered.servers[0].name, "alpha");
    }
}
