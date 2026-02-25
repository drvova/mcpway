pub mod registry;
pub mod sources;

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
}
