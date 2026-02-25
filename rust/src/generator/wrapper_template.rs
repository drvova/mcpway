use std::fmt::Write as _;

use crate::generator::normalize::NormalizedDefinition;

pub fn render_wrapper_source(normalized: &NormalizedDefinition, metadata_path: &str) -> String {
    let mut source = String::new();
    source.push_str("use std::env;\n");
    source.push_str("use std::path::PathBuf;\n");
    source.push_str("use std::process::{Command, Stdio};\n\n");

    source.push_str("const STDIO_COMMAND: &str = ");
    source.push_str(&rust_string_literal(&normalized.stdio_command));
    source.push_str(";\n");

    source.push_str("const METADATA_PATH: &str = ");
    source.push_str(&rust_string_literal(metadata_path));
    source.push_str(";\n\n");

    source.push_str("const ENV_BINDINGS: &[(&str, &str)] = &[\n");
    for binding in &normalized.env_bindings {
        let _ = writeln!(
            source,
            "    ({}, {}),",
            rust_string_literal(&binding.key),
            rust_string_literal(&binding.source_env)
        );
    }
    source.push_str("];\n\n");

    source.push_str("const HEADER_BINDINGS: &[(&str, &str)] = &[\n");
    for binding in &normalized.header_bindings {
        let _ = writeln!(
            source,
            "    ({}, {}),",
            rust_string_literal(&binding.header),
            rust_string_literal(&binding.source_env)
        );
    }
    source.push_str("];\n\n");

    source.push_str(
        r#"fn resolve_mcpway_bin() -> PathBuf {
    if let Ok(current) = env::current_exe() {
        if let Some(bin_dir) = current.parent() {
            let bundled_name = if cfg!(windows) { "mcpway.exe" } else { "mcpway" };
            let bundled = bin_dir.join(bundled_name);
            if bundled.exists() {
                return bundled;
            }
        }
    }
    PathBuf::from("mcpway")
}

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("Missing required environment variable: {key}"))
}

fn main() {
    let mut cmd = Command::new(resolve_mcpway_bin());
    cmd.arg("--stdio").arg(STDIO_COMMAND);

    for (key, env_key) in ENV_BINDINGS {
        match required_env(env_key) {
            Ok(value) => {
                cmd.arg("--env").arg(format!("{key}={value}"));
            }
            Err(err) => {
                eprintln!("[mcpway-wrapper] {err} (metadata: {METADATA_PATH})");
                std::process::exit(1);
            }
        }
    }

    for (header, env_key) in HEADER_BINDINGS {
        match required_env(env_key) {
            Ok(value) => {
                cmd.arg("--header").arg(format!("{header}: {value}"));
            }
            Err(err) => {
                eprintln!("[mcpway-wrapper] {err} (metadata: {METADATA_PATH})");
                std::process::exit(1);
            }
        }
    }

    cmd.args(env::args().skip(1));
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = match cmd.status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("[mcpway-wrapper] Failed to spawn mcpway: {err}");
            std::process::exit(1);
        }
    };

    std::process::exit(status.code().unwrap_or(1));
}
"#,
    );

    source
}

fn rust_string_literal(input: &str) -> String {
    format!("{:?}", input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::normalize::{EnvBinding, HeaderBinding, NormalizedDefinition};

    #[test]
    fn wrapper_template_contains_expected_constants() {
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

        let src = render_wrapper_source(&normalized, "/tmp/artifact/mcpway-artifact.json");
        assert!(src.contains("const STDIO_COMMAND"));
        assert!(src.contains("METADATA_PATH"));
        assert!(src.contains("MCPWAY_HEADER_AUTHORIZATION"));
    }
}
