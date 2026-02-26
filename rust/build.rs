fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure().compile_protos(&["proto/mcpway_bridge.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/mcpway_bridge.proto");
    Ok(())
}
