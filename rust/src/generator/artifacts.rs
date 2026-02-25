use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::generator::normalize::{required_env_keys, NormalizedDefinition};
use crate::generator::wrapper_template::render_wrapper_source;

#[derive(Debug, Clone)]
pub struct ArtifactBuildOptions {
    pub bundle_mcpway: bool,
    pub mcpway_binary: Option<PathBuf>,
    pub compile_wrapper: bool,
}

#[derive(Debug, Clone)]
pub struct GeneratedArtifacts {
    pub script_path: PathBuf,
    pub wrapper_path: Option<PathBuf>,
    pub mcpway_path: Option<PathBuf>,
    pub env_example_path: PathBuf,
}

pub fn build_artifacts(
    output_dir: &Path,
    normalized: &NormalizedDefinition,
    metadata_path: &Path,
    options: &ArtifactBuildOptions,
) -> Result<GeneratedArtifacts, String> {
    let bin_dir = output_dir.join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|err| format!("Failed to create {}: {err}", bin_dir.display()))?;

    let script_path = write_launcher_script(&bin_dir, normalized)?;
    let env_example_path = write_env_example(output_dir, normalized)?;

    let mcpway_path = if options.bundle_mcpway {
        let source = resolve_mcpway_binary_path(options.mcpway_binary.as_deref())?;
        let destination_name = if cfg!(windows) {
            "mcpway.exe"
        } else {
            "mcpway"
        };
        let destination = bin_dir.join(destination_name);
        fs::copy(&source, &destination).map_err(|err| {
            format!(
                "Failed to bundle mcpway from {} to {}: {err}",
                source.display(),
                destination.display()
            )
        })?;
        make_executable(&destination)?;
        Some(destination)
    } else {
        None
    };

    let wrapper_path = if options.compile_wrapper {
        Some(compile_wrapper_binary(
            output_dir,
            normalized,
            metadata_path,
            &bin_dir,
        )?)
    } else {
        None
    };

    Ok(GeneratedArtifacts {
        script_path,
        wrapper_path,
        mcpway_path,
        env_example_path,
    })
}

fn write_launcher_script(
    bin_dir: &Path,
    normalized: &NormalizedDefinition,
) -> Result<PathBuf, String> {
    let script_name = if cfg!(windows) {
        format!("{}.cmd", normalized.artifact_name)
    } else {
        normalized.artifact_name.clone()
    };
    let script_path = bin_dir.join(script_name);

    let script = if cfg!(windows) {
        render_windows_launcher(normalized)
    } else {
        render_posix_launcher(normalized)
    };

    fs::write(&script_path, script)
        .map_err(|err| format!("Failed to write launcher {}: {err}", script_path.display()))?;
    make_executable(&script_path)?;
    Ok(script_path)
}

fn write_env_example(
    output_dir: &Path,
    normalized: &NormalizedDefinition,
) -> Result<PathBuf, String> {
    let env_path = output_dir.join(".env.example");
    let mut keys = required_env_keys(normalized);
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

fn resolve_mcpway_binary_path(override_path: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = override_path {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(format!(
            "Provided --mcpway-binary does not exist: {}",
            path.display()
        ));
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(file_name) = current_exe.file_name().and_then(|v| v.to_str()) {
            if file_name.starts_with("mcpway") {
                return Ok(current_exe);
            }
        }
    }

    if let Some(path) = find_in_path(if cfg!(windows) {
        "mcpway.exe"
    } else {
        "mcpway"
    }) {
        return Ok(path);
    }

    Err("Could not locate mcpway binary for bundling; use --mcpway-binary".to_string())
}

fn find_in_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for entry in env::split_paths(&path_var) {
        let candidate = entry.join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn compile_wrapper_binary(
    output_dir: &Path,
    normalized: &NormalizedDefinition,
    metadata_path: &Path,
    bin_dir: &Path,
) -> Result<PathBuf, String> {
    let src_dir = output_dir.join(".wrapper-src");
    if src_dir.exists() {
        fs::remove_dir_all(&src_dir).map_err(|err| {
            format!(
                "Failed to clean wrapper source {}: {err}",
                src_dir.display()
            )
        })?;
    }
    fs::create_dir_all(src_dir.join("src")).map_err(|err| {
        format!(
            "Failed to create wrapper source {}: {err}",
            src_dir.display()
        )
    })?;

    let package_name = format!(
        "mcpway-wrapper-{}",
        normalized.artifact_name.replace('_', "-")
    );
    let bin_name = format!("{}-wrapper", normalized.artifact_name);

    let cargo_toml = format!(
        "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"{bin_name}\"\npath = \"src/main.rs\"\n"
    );
    fs::write(src_dir.join("Cargo.toml"), cargo_toml)
        .map_err(|err| format!("Failed to write wrapper Cargo.toml: {err}"))?;

    let metadata_literal = metadata_path.to_string_lossy().to_string();
    let wrapper_source = render_wrapper_source(normalized, &metadata_literal);
    fs::write(src_dir.join("src/main.rs"), wrapper_source)
        .map_err(|err| format!("Failed to write wrapper source: {err}"))?;

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg(&bin_name)
        .current_dir(&src_dir)
        .status()
        .map_err(|err| format!("Failed to launch cargo for wrapper build: {err}"))?;
    if !status.success() {
        return Err("Wrapper build failed".to_string());
    }

    let built_name = if cfg!(windows) {
        format!("{bin_name}.exe")
    } else {
        bin_name.clone()
    };
    let built_binary = src_dir.join("target").join("release").join(&built_name);
    if !built_binary.is_file() {
        return Err(format!(
            "Expected wrapper binary was not produced: {}",
            built_binary.display()
        ));
    }

    let destination = bin_dir.join(built_name);
    fs::copy(&built_binary, &destination).map_err(|err| {
        format!(
            "Failed to copy wrapper binary from {} to {}: {err}",
            built_binary.display(),
            destination.display()
        )
    })?;
    make_executable(&destination)?;

    Ok(destination)
}

fn render_posix_launcher(normalized: &NormalizedDefinition) -> String {
    let mut script = String::new();
    script.push_str("#!/usr/bin/env bash\n");
    script.push_str("set -euo pipefail\n\n");
    script.push_str("SELF_DIR=\"$(cd -- \"$(dirname -- \"${BASH_SOURCE[0]}\")\" && pwd)\"\n");
    script.push_str("if [[ -x \"${SELF_DIR}/mcpway\" ]]; then\n");
    script.push_str("  MCPWAY_BIN=\"${SELF_DIR}/mcpway\"\n");
    script.push_str("else\n");
    script.push_str("  MCPWAY_BIN=\"mcpway\"\n");
    script.push_str("fi\n\n");

    for binding in &normalized.env_bindings {
        script.push_str(&format!(
            "if [[ -z \"${{{}:-}}\" ]]; then echo \"Missing required environment variable: {}\" >&2; exit 1; fi\n",
            binding.source_env, binding.source_env
        ));
    }
    for binding in &normalized.header_bindings {
        script.push_str(&format!(
            "if [[ -z \"${{{}:-}}\" ]]; then echo \"Missing required environment variable: {}\" >&2; exit 1; fi\n",
            binding.source_env, binding.source_env
        ));
    }
    if !normalized.env_bindings.is_empty() || !normalized.header_bindings.is_empty() {
        script.push('\n');
    }

    script.push_str(&format!(
        "STDIO_CMD={}\n",
        shell_single_quote(&normalized.stdio_command)
    ));
    script.push_str("ARGS=(--stdio \"${STDIO_CMD}\")\n");

    for binding in &normalized.env_bindings {
        script.push_str(&format!(
            "ARGS+=(--env \"{}=${{{}}}\")\n",
            escape_for_double_quotes(&binding.key),
            binding.source_env
        ));
    }
    for binding in &normalized.header_bindings {
        script.push_str(&format!(
            "ARGS+=(--header \"{}: ${{{}}}\")\n",
            escape_for_double_quotes(&binding.header),
            binding.source_env
        ));
    }

    script.push_str("exec \"${MCPWAY_BIN}\" \"${ARGS[@]}\" \"$@\"\n");
    script
}

fn render_windows_launcher(normalized: &NormalizedDefinition) -> String {
    let mut script = String::new();
    script.push_str("@echo off\r\n");
    script.push_str("setlocal enabledelayedexpansion\r\n");
    script.push_str("set \"SELF_DIR=%~dp0\"\r\n");
    script.push_str("if exist \"%SELF_DIR%mcpway.exe\" (\r\n");
    script.push_str("  set \"MCPWAY_BIN=%SELF_DIR%mcpway.exe\"\r\n");
    script.push_str(") else (\r\n");
    script.push_str("  set \"MCPWAY_BIN=mcpway\"\r\n");
    script.push_str(")\r\n");

    for binding in &normalized.env_bindings {
        script.push_str(&format!(
            "if \"%{}%\"==\"\" ( echo Missing required environment variable: {} & exit /b 1 )\r\n",
            binding.source_env, binding.source_env
        ));
    }
    for binding in &normalized.header_bindings {
        script.push_str(&format!(
            "if \"%{}%\"==\"\" ( echo Missing required environment variable: {} & exit /b 1 )\r\n",
            binding.source_env, binding.source_env
        ));
    }

    script.push_str(&format!(
        "set \"STDIO_CMD={}\"\r\n",
        normalized.stdio_command.replace('"', "\\\"")
    ));

    script.push_str("\"%MCPWAY_BIN%\" --stdio \"%STDIO_CMD%\"");
    for binding in &normalized.env_bindings {
        script.push_str(&format!(
            " --env \"{}=%{}%\"",
            binding.key, binding.source_env
        ));
    }
    for binding in &normalized.header_bindings {
        script.push_str(&format!(
            " --header \"{}: %{}%\"",
            binding.header, binding.source_env
        ));
    }
    script.push_str(" %*\r\n");
    script
}

fn shell_single_quote(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }
    let escaped = input.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn escape_for_double_quotes(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::normalize::{EnvBinding, HeaderBinding, NormalizedDefinition};

    #[test]
    fn posix_script_contains_expected_arguments() {
        let normalized = NormalizedDefinition {
            artifact_name: "demo".to_string(),
            server_name: "demo".to_string(),
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            stdio_command: "node server.js".to_string(),
            env_bindings: vec![EnvBinding {
                key: "API_KEY".to_string(),
                source_env: "API_KEY".to_string(),
            }],
            header_bindings: vec![HeaderBinding {
                header: "Authorization".to_string(),
                source_env: "MCPWAY_HEADER_AUTHORIZATION".to_string(),
            }],
        };

        let script = render_posix_launcher(&normalized);
        assert!(script.contains("ARGS+=(--env \"API_KEY=${API_KEY}\")"));
        assert!(
            script.contains("ARGS+=(--header \"Authorization: ${MCPWAY_HEADER_AUTHORIZATION}\")")
        );
    }
}
