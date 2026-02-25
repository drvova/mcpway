use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactMode {
    #[default]
    Generate,
    Connect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    #[serde(default)]
    pub mode: ArtifactMode,
    pub schema_version: String,
    pub generated_at_utc: String,
    pub artifact_name: String,
    pub output_dir: String,
    pub source: SourceMetadata,
    pub normalized: NormalizedMetadata,
    pub generation: GenerationMetadata,
    pub artifacts: ArtifactPaths,
    pub regenerate_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectProfileMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub definition_path: String,
    pub server_selector: Option<String>,
    pub definition_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedMetadata {
    pub command: String,
    pub args: Vec<String>,
    pub env_template: BTreeMap<String, String>,
    pub headers_template: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetadata {
    pub options: GenerationOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationOptions {
    pub bundle_mcpway: bool,
    pub compile_wrapper: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPaths {
    pub script_path: String,
    pub wrapper_path: Option<String>,
    pub mcpway_path: Option<String>,
    pub env_example_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectProfileMetadata {
    pub endpoint: String,
    pub protocol: String,
    #[serde(default)]
    pub headers_template: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
}

pub fn write_metadata(path: &Path, metadata: &ArtifactMetadata) -> Result<(), String> {
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|err| format!("Failed to serialize metadata: {err}"))?;
    fs::write(path, json)
        .map_err(|err| format!("Failed to write metadata {}: {err}", path.display()))
}

pub fn read_metadata(path: &Path) -> Result<ArtifactMetadata, String> {
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read metadata {}: {err}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid metadata JSON in {}: {err}", path.display()))
}

pub fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn metadata_roundtrip() {
        let metadata = ArtifactMetadata {
            mode: ArtifactMode::Generate,
            schema_version: "1".to_string(),
            generated_at_utc: "2026-02-24T13:00:00Z".to_string(),
            artifact_name: "demo".to_string(),
            output_dir: "/tmp/demo".to_string(),
            source: SourceMetadata {
                definition_path: "/tmp/def.json".to_string(),
                server_selector: Some("serverA".to_string()),
                definition_sha256: "abc".to_string(),
            },
            normalized: NormalizedMetadata {
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                env_template: BTreeMap::from([("API_KEY".to_string(), "${API_KEY}".to_string())]),
                headers_template: BTreeMap::from([(
                    "Authorization".to_string(),
                    "${MCPWAY_HEADER_AUTHORIZATION}".to_string(),
                )]),
            },
            generation: GenerationMetadata {
                options: GenerationOptions {
                    bundle_mcpway: true,
                    compile_wrapper: true,
                },
            },
            artifacts: ArtifactPaths {
                script_path: "/tmp/demo/bin/demo".to_string(),
                wrapper_path: Some("/tmp/demo/bin/demo-wrapper".to_string()),
                mcpway_path: Some("/tmp/demo/bin/mcpway".to_string()),
                env_example_path: "/tmp/demo/.env.example".to_string(),
            },
            regenerate_command:
                "mcpway regenerate --metadata /tmp/demo/mcpway-artifact.json"
                    .to_string(),
            connect: None,
        };

        let mut path = std::env::temp_dir();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock drift")
            .as_nanos();
        path.push(format!("mcpway-metadata-test-{now}.json"));
        write_metadata(&path, &metadata).expect("metadata write failed");
        let read_back = read_metadata(&path).expect("metadata read failed");
        let _ = fs::remove_file(&path);
        assert_eq!(read_back.artifact_name, "demo");
        assert_eq!(read_back.schema_version, "1");
        assert!(read_back.generation.options.bundle_mcpway);
    }
}
