use std::fs;
use std::path::Path;

pub fn get_version() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let package_json = Path::new(manifest_dir).join("..").join("package.json");
    if let Ok(contents) = fs::read_to_string(package_json) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) {
            if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
                return version.to_string();
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}
