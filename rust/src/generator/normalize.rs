use std::collections::{BTreeMap, HashSet};

use crate::generator::definition::ParsedServerDefinition;

#[derive(Debug, Clone)]
pub struct EnvBinding {
    pub key: String,
    pub source_env: String,
}

#[derive(Debug, Clone)]
pub struct HeaderBinding {
    pub header: String,
    pub source_env: String,
}

#[derive(Debug, Clone)]
pub struct NormalizedDefinition {
    pub artifact_name: String,
    pub server_name: String,
    pub command: String,
    pub args: Vec<String>,
    pub stdio_command: String,
    pub env_bindings: Vec<EnvBinding>,
    pub header_bindings: Vec<HeaderBinding>,
}

pub fn normalize_definition(
    parsed: &ParsedServerDefinition,
    artifact_name_override: Option<&str>,
) -> Result<NormalizedDefinition, String> {
    let artifact_name = artifact_name_override
        .map(sanitize_artifact_name)
        .unwrap_or_else(|| sanitize_artifact_name(&parsed.server_name));
    if artifact_name.is_empty() {
        return Err("Artifact name resolved to empty value".to_string());
    }

    let stdio_command = {
        let mut pieces = Vec::with_capacity(1 + parsed.args.len());
        pieces.push(parsed.command.clone());
        pieces.extend(parsed.args.clone());
        shell_words::join(pieces)
    };

    let env_bindings = parsed
        .env
        .keys()
        .map(|key| EnvBinding {
            key: key.to_string(),
            source_env: key.to_string(),
        })
        .collect::<Vec<_>>();

    let mut used_header_env_vars: HashSet<String> = HashSet::new();
    let mut header_bindings = Vec::new();
    for header_name in parsed.headers.keys() {
        let base = format!("MCPWAY_HEADER_{}", sanitize_env_key(header_name));
        let mut candidate = base.clone();
        let mut suffix = 1usize;
        while used_header_env_vars.contains(&candidate) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        used_header_env_vars.insert(candidate.clone());
        header_bindings.push(HeaderBinding {
            header: header_name.to_string(),
            source_env: candidate,
        });
    }

    Ok(NormalizedDefinition {
        artifact_name,
        server_name: parsed.server_name.clone(),
        command: parsed.command.clone(),
        args: parsed.args.clone(),
        stdio_command,
        env_bindings,
        header_bindings,
    })
}

pub fn env_template_map(normalized: &NormalizedDefinition) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for binding in &normalized.env_bindings {
        out.insert(binding.key.clone(), format!("${{{}}}", binding.source_env));
    }
    out
}

pub fn header_template_map(normalized: &NormalizedDefinition) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for binding in &normalized.header_bindings {
        out.insert(
            binding.header.clone(),
            format!("${{{}}}", binding.source_env),
        );
    }
    out
}

pub fn required_env_keys(normalized: &NormalizedDefinition) -> Vec<String> {
    let mut keys = Vec::new();
    for binding in &normalized.env_bindings {
        keys.push(binding.source_env.clone());
    }
    for binding in &normalized.header_bindings {
        keys.push(binding.source_env.clone());
    }
    keys.sort();
    keys.dedup();
    keys
}

pub fn sanitize_artifact_name(raw: &str) -> String {
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
    out.trim_matches('-').to_string()
}

fn sanitize_env_key(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::definition::ParsedServerDefinition;

    #[test]
    fn normalize_builds_template_bindings() {
        let parsed = ParsedServerDefinition {
            server_name: "My Server".to_string(),
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env: BTreeMap::from([("API_KEY".to_string(), "secret".to_string())]),
            headers: BTreeMap::from([("Authorization".to_string(), "Bearer abc".to_string())]),
        };

        let normalized = normalize_definition(&parsed, None).expect("normalize should succeed");
        assert_eq!(normalized.artifact_name, "my-server");
        assert_eq!(normalized.stdio_command, "node server.js");

        let env_tpl = env_template_map(&normalized);
        assert_eq!(env_tpl.get("API_KEY"), Some(&"${API_KEY}".to_string()));

        let header_tpl = header_template_map(&normalized);
        assert_eq!(
            header_tpl.get("Authorization"),
            Some(&"${MCPWAY_HEADER_AUTHORIZATION}".to_string())
        );
    }
}
