mod artifacts;
mod definition;
mod metadata;
mod normalize;
mod wrapper_template;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{ConnectConfig, ConnectProtocol, GenerateConfig, RegenerateConfig};
use crate::generator::artifacts::{build_artifacts, ArtifactBuildOptions, GeneratedArtifacts};
use crate::generator::definition::load_definition;
use crate::generator::metadata::{
    read_metadata, sha256_hex, write_metadata, ArtifactMetadata, ArtifactMode, ArtifactPaths,
    ConnectProfileMetadata, GenerationMetadata, GenerationOptions, NormalizedMetadata,
    SourceMetadata,
};
use crate::generator::normalize::{
    env_template_map, header_template_map, normalize_definition, sanitize_artifact_name,
    NormalizedDefinition,
};

pub fn run_generate(config: &GenerateConfig) -> Result<(), String> {
    let definition_path = absolute_path(&config.definition)?;
    let output_dir = absolute_path(&config.out)?;

    let parsed = load_definition(&definition_path, config.server.as_deref())?;
    let normalized = normalize_definition(&parsed, config.artifact_name.as_deref())?;

    fs::create_dir_all(&output_dir)
        .map_err(|err| format!("Failed to create {}: {err}", output_dir.display()))?;

    let metadata_path = output_dir.join("mcpway-artifact.json");
    let artifact_outputs = build_artifacts(
        &output_dir,
        &normalized,
        &metadata_path,
        &ArtifactBuildOptions {
            bundle_mcpway: config.bundle_mcpway,
            mcpway_binary: config.mcpway_binary.clone(),
            compile_wrapper: config.compile_wrapper,
        },
    )?;

    let definition_bytes = fs::read(&definition_path).map_err(|err| {
        format!(
            "Failed to read definition {}: {err}",
            definition_path.display()
        )
    })?;

    let metadata = build_metadata(MetadataBuildInput {
        normalized: &normalized,
        definition_path: &definition_path,
        server_selector: Some(normalized.server_name.clone()),
        definition_bytes: &definition_bytes,
        output_dir: &output_dir,
        options: GenerationOptions {
            bundle_mcpway: config.bundle_mcpway,
            compile_wrapper: config.compile_wrapper,
        },
        artifacts: &artifact_outputs,
    });

    write_metadata(&metadata_path, &metadata)?;

    println!(
        "[mcpway] Generated artifact: {}",
        output_dir.display()
    );
    println!(
        "[mcpway] Regenerate with: {}",
        metadata.regenerate_command
    );

    Ok(())
}

pub fn save_connect_profile(
    config: &ConnectConfig,
    protocol: ConnectProtocol,
) -> Result<(), String> {
    let spec = connect_profile_from_config(config, protocol)?;
    let metadata = write_connect_profile(&spec)?;

    println!(
        "[mcpway] Saved connect profile: {}",
        spec.output_dir.display()
    );
    println!(
        "[mcpway] Regenerate with: {}",
        metadata.regenerate_command
    );

    Ok(())
}

pub fn run_regenerate(config: &RegenerateConfig) -> Result<(), String> {
    let metadata_path = absolute_path(&config.metadata)?;
    let existing = read_metadata(&metadata_path)?;

    if existing.mode == ArtifactMode::Connect {
        let spec = connect_profile_from_metadata(&existing, config.out.as_deref())?;
        let metadata = write_connect_profile(&spec)?;
        println!(
            "[mcpway] Regenerated connect profile: {}",
            spec.output_dir.display()
        );
        println!(
            "[mcpway] Regenerate with: {}",
            metadata.regenerate_command
        );
        return Ok(());
    }

    let definition_path = if let Some(path) = &config.definition {
        absolute_path(path)?
    } else {
        PathBuf::from(existing.source.definition_path.clone())
    };

    let out = if let Some(path) = &config.out {
        absolute_path(path)?
    } else {
        PathBuf::from(existing.output_dir.clone())
    };

    let bundle_mcpway = config
        .bundle_mcpway
        .unwrap_or(existing.generation.options.bundle_mcpway);
    let compile_wrapper = config
        .compile_wrapper
        .unwrap_or(existing.generation.options.compile_wrapper);

    let generate_config = GenerateConfig {
        definition: definition_path,
        server: config
            .server
            .clone()
            .or(existing.source.server_selector.clone()),
        out,
        artifact_name: Some(existing.artifact_name.clone()),
        bundle_mcpway,
        mcpway_binary: config.mcpway_binary.clone(),
        compile_wrapper,
    };

    run_generate(&generate_config)
}

struct MetadataBuildInput<'a> {
    normalized: &'a NormalizedDefinition,
    definition_path: &'a Path,
    server_selector: Option<String>,
    definition_bytes: &'a [u8],
    output_dir: &'a Path,
    options: GenerationOptions,
    artifacts: &'a GeneratedArtifacts,
}

fn build_metadata(input: MetadataBuildInput<'_>) -> ArtifactMetadata {
    let metadata_path = input.output_dir.join("mcpway-artifact.json");
    ArtifactMetadata {
        mode: ArtifactMode::Generate,
        schema_version: "1".to_string(),
        generated_at_utc: unix_timestamp_utc_string(),
        artifact_name: input.normalized.artifact_name.clone(),
        output_dir: input.output_dir.to_string_lossy().to_string(),
        source: SourceMetadata {
            definition_path: input.definition_path.to_string_lossy().to_string(),
            server_selector: input.server_selector,
            definition_sha256: sha256_hex(input.definition_bytes),
        },
        normalized: NormalizedMetadata {
            command: input.normalized.command.clone(),
            args: input.normalized.args.clone(),
            env_template: env_template_map(input.normalized),
            headers_template: header_template_map(input.normalized),
        },
        generation: GenerationMetadata {
            options: input.options,
        },
        artifacts: ArtifactPaths {
            script_path: input.artifacts.script_path.to_string_lossy().to_string(),
            wrapper_path: input
                .artifacts
                .wrapper_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            mcpway_path: input
                .artifacts
                .mcpway_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            env_example_path: input
                .artifacts
                .env_example_path
                .to_string_lossy()
                .to_string(),
        },
        regenerate_command: format!(
            "mcpway regenerate --metadata {}",
            shell_words::quote(&metadata_path.to_string_lossy())
        ),
        connect: None,
    }
}

#[derive(Debug, Clone)]
struct ConnectProfileSpec {
    artifact_name: String,
    endpoint: String,
    protocol: String,
    header_bindings: Vec<(String, String)>,
    output_dir: PathBuf,
}

fn connect_profile_from_config(
    config: &ConnectConfig,
    protocol: ConnectProtocol,
) -> Result<ConnectProfileSpec, String> {
    let endpoint = config
        .endpoint
        .as_deref()
        .ok_or_else(|| "connect profile endpoint is missing".to_string())?
        .to_string();
    let raw_output_dir = config
        .save_profile_dir
        .as_ref()
        .ok_or_else(|| "save profile path missing".to_string())?;
    let output_dir = absolute_path(raw_output_dir)?;

    let derived_name = derive_endpoint_profile_name(&endpoint);
    let artifact_name = config
        .profile_name
        .as_ref()
        .map(|name| sanitize_artifact_name(name))
        .filter(|name| !name.is_empty())
        .unwrap_or(derived_name);

    let mut header_bindings = Vec::new();
    let mut headers: Vec<_> = config.headers.keys().cloned().collect();
    headers.sort();
    for header in headers {
        header_bindings.push((header.clone(), header_env_var_name(&header)));
    }

    Ok(ConnectProfileSpec {
        artifact_name,
        endpoint,
        protocol: protocol.as_str().to_string(),
        header_bindings,
        output_dir,
    })
}

fn connect_profile_from_metadata(
    metadata: &ArtifactMetadata,
    out_override: Option<&Path>,
) -> Result<ConnectProfileSpec, String> {
    let connect = metadata
        .connect
        .as_ref()
        .ok_or("connect metadata missing for connect profile regeneration")?;

    let output_dir = if let Some(path) = out_override {
        absolute_path(path)?
    } else {
        PathBuf::from(metadata.output_dir.clone())
    };

    let artifact_name = sanitize_artifact_name(&metadata.artifact_name);
    if artifact_name.is_empty() {
        return Err("Invalid artifact name in metadata".to_string());
    }

    let mut header_bindings = Vec::new();
    for (header, template) in &connect.headers_template {
        let env_key =
            extract_template_env_key(template).unwrap_or_else(|| header_env_var_name(header));
        header_bindings.push((header.clone(), env_key));
    }
    header_bindings.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(ConnectProfileSpec {
        artifact_name,
        endpoint: connect.endpoint.clone(),
        protocol: connect.protocol.clone(),
        header_bindings,
        output_dir,
    })
}

fn write_connect_profile(spec: &ConnectProfileSpec) -> Result<ArtifactMetadata, String> {
    let bin_dir = spec.output_dir.join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|err| format!("Failed to create {}: {err}", bin_dir.display()))?;

    let script_path = write_connect_profile_script(&bin_dir, spec)?;
    let env_example_path = write_connect_env_example(&spec.output_dir, spec)?;
    let metadata_path = spec.output_dir.join("mcpway-artifact.json");

    let mut headers_template = BTreeMap::new();
    for (header, env_key) in &spec.header_bindings {
        headers_template.insert(header.clone(), format!("${{{env_key}}}"));
    }

    let metadata = ArtifactMetadata {
        mode: ArtifactMode::Connect,
        schema_version: "1".to_string(),
        generated_at_utc: unix_timestamp_utc_string(),
        artifact_name: spec.artifact_name.clone(),
        output_dir: spec.output_dir.to_string_lossy().to_string(),
        source: SourceMetadata {
            definition_path: "".to_string(),
            server_selector: None,
            definition_sha256: "".to_string(),
        },
        normalized: NormalizedMetadata {
            command: "".to_string(),
            args: Vec::new(),
            env_template: BTreeMap::new(),
            headers_template: headers_template.clone(),
        },
        generation: GenerationMetadata {
            options: GenerationOptions {
                bundle_mcpway: false,
                compile_wrapper: false,
            },
        },
        artifacts: ArtifactPaths {
            script_path: script_path.to_string_lossy().to_string(),
            wrapper_path: None,
            mcpway_path: None,
            env_example_path: env_example_path.to_string_lossy().to_string(),
        },
        regenerate_command: format!(
            "mcpway regenerate --metadata {}",
            shell_words::quote(&metadata_path.to_string_lossy())
        ),
        connect: Some(ConnectProfileMetadata {
            endpoint: spec.endpoint.clone(),
            protocol: spec.protocol.clone(),
            headers_template,
            profile_name: Some(spec.artifact_name.clone()),
        }),
    };

    write_metadata(&metadata_path, &metadata)?;
    Ok(metadata)
}

fn write_connect_profile_script(
    bin_dir: &Path,
    spec: &ConnectProfileSpec,
) -> Result<PathBuf, String> {
    let script_name = if cfg!(windows) {
        format!("{}.cmd", spec.artifact_name)
    } else {
        spec.artifact_name.clone()
    };
    let script_path = bin_dir.join(script_name);

    let script = if cfg!(windows) {
        render_windows_connect_profile_script(spec)
    } else {
        render_posix_connect_profile_script(spec)
    };

    fs::write(&script_path, script).map_err(|err| {
        format!(
            "Failed to write profile script {}: {err}",
            script_path.display()
        )
    })?;
    make_executable(&script_path)?;

    Ok(script_path)
}

fn render_posix_connect_profile_script(spec: &ConnectProfileSpec) -> String {
    let mut script = String::new();
    script.push_str("#!/usr/bin/env bash\n");
    script.push_str("set -euo pipefail\n\n");

    for (_header, env_key) in &spec.header_bindings {
        script.push_str(&format!(
            "if [[ -z \"${{{env_key}:-}}\" ]]; then echo \"Missing required environment variable: {env_key}\" >&2; exit 1; fi\n"
        ));
    }
    if !spec.header_bindings.is_empty() {
        script.push('\n');
    }

    script.push_str("ARGS=(connect");
    script.push_str(&format!(" {}", shell_single_quote(&spec.endpoint)));
    script.push_str(&format!(
        " --protocol {})\n",
        shell_single_quote(&spec.protocol)
    ));

    for (header, env_key) in &spec.header_bindings {
        script.push_str(&format!(
            "ARGS+=(--header \"{}: ${{{}}}\")\n",
            escape_for_double_quotes(header),
            env_key
        ));
    }

    script.push_str("exec mcpway \"${ARGS[@]}\" \"$@\"\n");
    script
}

fn render_windows_connect_profile_script(spec: &ConnectProfileSpec) -> String {
    let mut script = String::new();
    script.push_str("@echo off\r\n");
    script.push_str("setlocal enabledelayedexpansion\r\n");

    for (_header, env_key) in &spec.header_bindings {
        script.push_str(&format!(
            "if \"%{}%\"==\"\" ( echo Missing required environment variable: {} & exit /b 1 )\r\n",
            env_key, env_key
        ));
    }

    script.push_str(&format!(
        "mcpway connect \"{}\" --protocol \"{}\"",
        spec.endpoint.replace('"', "\\\""),
        spec.protocol.replace('"', "\\\"")
    ));

    for (header, env_key) in &spec.header_bindings {
        script.push_str(&format!(
            " --header \"{}: %{}%\"",
            header.replace('"', "\\\""),
            env_key
        ));
    }
    script.push_str(" %*\r\n");

    script
}

fn write_connect_env_example(
    output_dir: &Path,
    spec: &ConnectProfileSpec,
) -> Result<PathBuf, String> {
    let env_path = output_dir.join(".env.example");
    let mut keys: Vec<String> = spec
        .header_bindings
        .iter()
        .map(|(_header, env_key)| env_key.clone())
        .collect();
    keys.sort();
    keys.dedup();

    let mut body = String::new();
    for key in keys {
        body.push_str(&format!("{key}=\n"));
    }

    fs::write(&env_path, body)
        .map_err(|err| format!("Failed to write {}: {err}", env_path.display()))?;
    Ok(env_path)
}

fn derive_endpoint_profile_name(endpoint: &str) -> String {
    if let Ok(url) = url::Url::parse(endpoint) {
        if let Some(host) = url.host_str() {
            let name = sanitize_artifact_name(host);
            if !name.is_empty() {
                return name;
            }
        }
    }
    "endpoint-profile".to_string()
}

fn header_env_var_name(header_name: &str) -> String {
    let mut out = String::from("MCPWAY_CONNECT_HEADER_");
    for ch in header_name.chars() {
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

fn extract_template_env_key(value: &str) -> Option<String> {
    if value.starts_with("${") && value.ends_with('}') && value.len() > 3 {
        return Some(value[2..value.len() - 1].to_string());
    }
    None
}

fn shell_single_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn escape_for_double_quotes(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|err| format!("Failed to stat {}: {err}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).map_err(|err| {
            format!(
                "Failed to set executable permissions on {}: {err}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = env::current_dir().map_err(|err| format!("Failed to read cwd: {err}"))?;
    Ok(cwd.join(path))
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

    #[test]
    fn builds_metadata_record() {
        let normalized = NormalizedDefinition {
            artifact_name: "demo".to_string(),
            server_name: "demo".to_string(),
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            stdio_command: "node server.js".to_string(),
            env_bindings: Vec::new(),
            header_bindings: Vec::new(),
        };

        let output_dir = PathBuf::from("/tmp/mcpway-demo");
        let artifacts = GeneratedArtifacts {
            script_path: PathBuf::from("/tmp/mcpway-demo/bin/demo"),
            wrapper_path: None,
            mcpway_path: None,
            env_example_path: PathBuf::from("/tmp/mcpway-demo/.env.example"),
        };
        let metadata = build_metadata(MetadataBuildInput {
            normalized: &normalized,
            definition_path: Path::new("/tmp/def.json"),
            server_selector: Some("demo".to_string()),
            definition_bytes: b"{}",
            output_dir: &output_dir,
            options: GenerationOptions {
                bundle_mcpway: true,
                compile_wrapper: false,
            },
            artifacts: &artifacts,
        });

        assert_eq!(metadata.mode, ArtifactMode::Generate);
        assert_eq!(metadata.schema_version, "1");
        assert_eq!(metadata.artifact_name, "demo");
        assert!(metadata
            .regenerate_command
            .contains("regenerate --metadata"));
    }

    #[test]
    fn connect_profile_extracts_template_env_key() {
        assert_eq!(
            extract_template_env_key("${MCPWAY_CONNECT_HEADER_AUTHORIZATION}"),
            Some("MCPWAY_CONNECT_HEADER_AUTHORIZATION".to_string())
        );
        assert_eq!(extract_template_env_key("plain"), None);
    }
}
