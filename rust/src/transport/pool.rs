use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::types::HeadersMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WarmHint {
    transport: String,
    last_success_utc: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WarmHintStore {
    records: BTreeMap<String, WarmHint>,
}

#[derive(Default)]
pub struct TransportPool {
    http_clients: Mutex<HashMap<String, Arc<reqwest::Client>>>,
    warm_hints: Mutex<WarmHintStore>,
}

static GLOBAL_POOL: OnceLock<Arc<TransportPool>> = OnceLock::new();

pub fn global_pool() -> Arc<TransportPool> {
    GLOBAL_POOL
        .get_or_init(|| Arc::new(TransportPool::new()))
        .clone()
}

impl TransportPool {
    fn new() -> Self {
        Self {
            http_clients: Mutex::new(HashMap::new()),
            warm_hints: Mutex::new(load_warm_hint_store().unwrap_or_default()),
        }
    }

    pub async fn http_client(
        &self,
        key: &str,
        connect_timeout: Duration,
        request_timeout: Option<Duration>,
    ) -> Result<Arc<reqwest::Client>, String> {
        {
            let clients = self.http_clients.lock().await;
            if let Some(existing) = clients.get(key) {
                return Ok(existing.clone());
            }
        }

        {
            let hints = self.warm_hints.lock().await;
            if hints.records.contains_key(key) {
                tracing::debug!("Using warm transport hint for key={key}");
            }
        }

        let mut builder = reqwest::Client::builder().connect_timeout(connect_timeout);
        if let Some(timeout) = request_timeout {
            builder = builder.timeout(timeout);
        }

        let client = builder
            .build()
            .map_err(|err| format!("Failed to build HTTP client: {err}"))?;
        let client = Arc::new(client);

        let mut clients = self.http_clients.lock().await;
        clients.insert(key.to_string(), client.clone());
        Ok(client)
    }

    pub async fn mark_success(&self, key: &str, transport: &str) {
        let mut hints = self.warm_hints.lock().await;
        hints.records.insert(
            key.to_string(),
            WarmHint {
                transport: transport.to_string(),
                last_success_utc: unix_timestamp_secs(),
            },
        );

        if let Err(err) = persist_warm_hint_store(&hints) {
            tracing::warn!("Failed to persist transport warm cache: {err}");
        }
    }
}

pub fn transport_fingerprint(
    transport: &str,
    endpoint_or_command: &str,
    headers: &HeadersMap,
    protocol_version: &str,
) -> String {
    let mut pairs = headers
        .iter()
        .map(|(k, v)| format!("{}={}", k.to_ascii_lowercase(), v))
        .collect::<Vec<_>>();
    pairs.sort();

    let mut payload = String::new();
    payload.push_str(transport);
    payload.push('|');
    payload.push_str(endpoint_or_command);
    payload.push('|');
    payload.push_str(protocol_version);
    payload.push('|');
    payload.push_str(&pairs.join(";"));

    sha256_hex(payload.as_bytes())
}

fn warm_cache_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MCPWAY_WARM_CACHE_PATH") {
        return PathBuf::from(path);
    }

    if let Some(home) = crate::discovery::user_home_dir() {
        return home.join(".mcpway").join("transport-warm-cache.json");
    }

    PathBuf::from(".mcpway/transport-warm-cache.json")
}

fn load_warm_hint_store() -> Result<WarmHintStore, String> {
    let path = warm_cache_path();
    if !path.exists() {
        return Ok(WarmHintStore::default());
    }

    let body = std::fs::read_to_string(&path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    serde_json::from_str::<WarmHintStore>(&body)
        .map_err(|err| format!("Invalid warm cache JSON in {}: {err}", path.display()))
}

fn persist_warm_hint_store(store: &WarmHintStore) -> Result<(), String> {
    let path = warm_cache_path();
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid warm cache path: {}", path.display()))?;

    std::fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;

    let body = serde_json::to_string_pretty(store)
        .map_err(|err| format!("Failed to serialize warm cache: {err}"))?;
    std::fs::write(&path, body).map_err(|err| format!("Failed to write {}: {err}", path.display()))
}

fn unix_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_sensitive_to_inputs() {
        let mut headers = HeadersMap::new();
        headers.insert("Authorization".to_string(), "Bearer token-a".to_string());

        let first = transport_fingerprint(
            "streamable-http",
            "https://example.com/mcp",
            &headers,
            "2024-11-05",
        );
        let second = transport_fingerprint(
            "streamable-http",
            "https://example.com/mcp",
            &headers,
            "2024-11-05",
        );
        assert_eq!(first, second);

        headers.insert("X-Test".to_string(), "changed".to_string());
        let third = transport_fingerprint(
            "streamable-http",
            "https://example.com/mcp",
            &headers,
            "2024-11-05",
        );
        assert_ne!(first, third);
    }

    #[tokio::test]
    async fn warm_cache_persists_only_hashed_keys() {
        let tmp_path = std::env::temp_dir().join(format!(
            "mcpway-warm-cache-{}.json",
            std::process::id()
        ));
        std::env::set_var("MCPWAY_WARM_CACHE_PATH", &tmp_path);

        let pool = TransportPool::new();
        let key = "deadbeef";
        pool.mark_success(key, "streamable-http").await;

        let body = std::fs::read_to_string(&tmp_path).expect("warm cache file missing");
        assert!(body.contains("deadbeef"));
        assert!(!body.contains("https://example.com"));

        let _ = std::fs::remove_file(&tmp_path);
        std::env::remove_var("MCPWAY_WARM_CACHE_PATH");
    }
}
