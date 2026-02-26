fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let embedded_dist = manifest_dir.join("web-dist");
    let workspace_web = manifest_dir.join("../web");
    let workspace_dist = workspace_web.join("dist");
    let out_dist = std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("web-dist");

    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc_path);
    tonic_prost_build::configure().compile_protos(&["proto/mcpway_bridge.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/mcpway_bridge.proto");
    println!("cargo:rerun-if-changed=web-dist");
    println!("cargo:rerun-if-changed=../web/dist");
    println!("cargo:rerun-if-changed=../web/index.html");
    println!("cargo:rerun-if-changed=../web/src");
    println!("cargo:rerun-if-changed=../web/package.json");
    println!("cargo:rerun-if-changed=../web/package-lock.json");
    println!("cargo:rerun-if-env-changed=MCPWAY_BUILD_WEB");

    if std::env::var("MCPWAY_BUILD_WEB").ok().as_deref() == Some("1") {
        let install = std::process::Command::new("npm")
            .args(["ci"])
            .current_dir(&workspace_web)
            .status()?;
        if !install.success() {
            return Err("npm ci failed while building web assets".into());
        }

        let build = std::process::Command::new("npm")
            .args(["run", "build"])
            .current_dir(&workspace_web)
            .status()?;
        if !build.success() {
            return Err("npm run build failed while building web assets".into());
        }
    }

    let source_dist = if workspace_dist.join("index.html").is_file() {
        workspace_dist.as_path()
    } else {
        embedded_dist.as_path()
    };

    if !source_dist.join("index.html").is_file() {
        return Err(
            "Missing embedded web assets. Expected web/dist or rust/web-dist with index.html."
                .into(),
        );
    }

    sync_web_dist(source_dist, &out_dist)?;

    Ok(())
}

fn sync_web_dist(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if !source.is_dir() {
        return Err(format!("Expected web dist directory at {}", source.display()).into());
    }

    if destination.exists() {
        std::fs::remove_dir_all(destination)?;
    }
    std::fs::create_dir_all(destination)?;

    copy_dir_recursive(source, destination)?;
    Ok(())
}

fn copy_dir_recursive(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&destination_path)?;
            copy_dir_recursive(&source_path, &destination_path)?;
            continue;
        }
        if file_type.is_file() {
            std::fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}
