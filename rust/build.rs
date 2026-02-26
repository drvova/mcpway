fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc_path);
    tonic_prost_build::configure().compile_protos(&["proto/mcpway_bridge.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/mcpway_bridge.proto");
    Ok(())
}
