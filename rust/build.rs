fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc_path);
    tonic_prost_build::configure().compile_protos(&["proto/mcpway_bridge.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/mcpway_bridge.proto");
    println!("cargo:rerun-if-changed=../web/dist");
    println!("cargo:rerun-if-changed=../web/index.html");
    println!("cargo:rerun-if-changed=../web/src");
    println!("cargo:rerun-if-changed=../web/package.json");
    println!("cargo:rerun-if-changed=../web/package-lock.json");
    println!("cargo:rerun-if-env-changed=MCPWAY_BUILD_WEB");

    if std::env::var("MCPWAY_BUILD_WEB").ok().as_deref() == Some("1") {
        let install = std::process::Command::new("npm")
            .args(["ci"])
            .current_dir("../web")
            .status()?;
        if !install.success() {
            return Err("npm ci failed while building web assets".into());
        }

        let build = std::process::Command::new("npm")
            .args(["run", "build"])
            .current_dir("../web")
            .status()?;
        if !build.success() {
            return Err("npm run build failed while building web assets".into());
        }
    }

    Ok(())
}
