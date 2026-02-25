use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout, Duration};
use url::Url;
use uuid::Uuid;

use crate::config::{ConnectOauthConfig, OAuthFlow};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OAuthCacheStore {
    #[serde(default = "default_schema_version")]
    schema_version: String,
    #[serde(default)]
    records: BTreeMap<String, TokenRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at_utc: Option<u64>,
    #[serde(default)]
    obtained_at_utc: u64,
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OidcDiscovery {
    token_endpoint: String,
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TokenSuccessResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Clone)]
struct TokenGrant {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    expires_at_utc: Option<u64>,
    scope: String,
}

pub async fn resolve_access_token(config: &ConnectOauthConfig) -> Result<String, String> {
    let cache_path = config.cache_path.clone().unwrap_or_else(default_cache_path);
    let profile_key = profile_fingerprint(config);
    let now = unix_timestamp_secs();
    let mut store = load_store(&cache_path)?;

    if !config.login {
        if let Some(existing) = store.records.get(&profile_key) {
            if token_is_valid(existing, now) {
                return Ok(existing.access_token.clone());
            }
        }
    }

    let client = reqwest::Client::new();
    let discovery = discover_provider(&client, &config.issuer).await?;

    if !config.login {
        if let Some(existing) = store.records.get(&profile_key) {
            if let Some(refresh_token) = existing.refresh_token.as_deref() {
                if let Ok(grant) =
                    refresh_access_token(&client, &discovery, config, refresh_token).await
                {
                    let updated = build_record(&grant, config);
                    let access = updated.access_token.clone();
                    store.records.insert(profile_key.clone(), updated);
                    save_store(&cache_path, &store)?;
                    return Ok(access);
                }
            }
        }
    }

    let grant = match config.flow {
        OAuthFlow::Device => run_device_flow(&client, &discovery, config).await?,
        OAuthFlow::AuthCode => run_auth_code_flow(&client, &discovery, config).await?,
    };
    let record = build_record(&grant, config);
    let access = record.access_token.clone();
    store.records.insert(profile_key, record);
    save_store(&cache_path, &store)?;
    Ok(access)
}

pub fn logout(config: &ConnectOauthConfig) -> Result<usize, String> {
    let cache_path = config.cache_path.clone().unwrap_or_else(default_cache_path);
    let profile_key = profile_fingerprint(config);
    let mut store = load_store(&cache_path)?;
    let removed = if store.records.remove(&profile_key).is_some() {
        1usize
    } else {
        0usize
    };
    save_store(&cache_path, &store)?;
    Ok(removed)
}

pub fn default_cache_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MCPWAY_OAUTH_CACHE_PATH") {
        return PathBuf::from(path);
    }
    if let Some(home) = crate::discovery::user_home_dir() {
        return home.join(".mcpway").join("oauth-cache.json");
    }
    PathBuf::from(".mcpway/oauth-cache.json")
}

fn discover_url(issuer: &str) -> String {
    format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    )
}

async fn discover_provider(
    client: &reqwest::Client,
    issuer: &str,
) -> Result<OidcDiscovery, String> {
    let discovery_url = discover_url(issuer);
    let response = client
        .get(&discovery_url)
        .send()
        .await
        .map_err(|err| format!("Failed to fetch OIDC discovery from {discovery_url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "OIDC discovery failed at {discovery_url} with status {}",
            response.status()
        ));
    }
    response
        .json::<OidcDiscovery>()
        .await
        .map_err(|err| format!("Invalid OIDC discovery payload from {discovery_url}: {err}"))
}

async fn refresh_access_token(
    client: &reqwest::Client,
    discovery: &OidcDiscovery,
    config: &ConnectOauthConfig,
    refresh_token: &str,
) -> Result<TokenGrant, String> {
    let mut form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("client_id".to_string(), config.client_id.clone()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    let scope = normalized_scope(config.scopes.clone());
    if !scope.is_empty() {
        form.push(("scope".to_string(), scope));
    }
    if let Some(audience) = config.audience.as_deref() {
        form.push(("audience".to_string(), audience.to_string()));
    }

    let response = client
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|err| format!("OAuth refresh request failed: {err}"))?;
    parse_token_response(response).await
}

async fn run_device_flow(
    client: &reqwest::Client,
    discovery: &OidcDiscovery,
    config: &ConnectOauthConfig,
) -> Result<TokenGrant, String> {
    let endpoint = discovery
        .device_authorization_endpoint
        .clone()
        .unwrap_or_else(|| format!("{}/oauth/device/code", config.issuer.trim_end_matches('/')));
    let scope = normalized_scope(config.scopes.clone());
    let mut form = vec![("client_id".to_string(), config.client_id.clone())];
    if !scope.is_empty() {
        form.push(("scope".to_string(), scope));
    }
    if let Some(audience) = config.audience.as_deref() {
        form.push(("audience".to_string(), audience.to_string()));
    }

    let response = client
        .post(&endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|err| format!("OAuth device authorization request failed: {err}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "OAuth device authorization failed with status {status}: {body}"
        ));
    }
    let payload = response
        .json::<DeviceCodeResponse>()
        .await
        .map_err(|err| format!("Invalid device authorization response: {err}"))?;

    println!("[mcpway] OAuth device login required");
    println!(
        "[mcpway] Visit: {}",
        payload
            .verification_uri_complete
            .as_deref()
            .unwrap_or(&payload.verification_uri)
    );
    println!("[mcpway] Enter code: {}", payload.user_code);

    if !config.no_browser {
        if let Some(url) = payload.verification_uri_complete.as_deref() {
            let _ = try_open_browser(url);
        }
    }

    let mut interval = payload.interval.unwrap_or(5).max(1);
    let expires_in = payload.expires_in.unwrap_or(600);
    let deadline = unix_timestamp_secs().saturating_add(expires_in);

    loop {
        if unix_timestamp_secs() >= deadline {
            return Err("OAuth device flow timed out before authorization completed".to_string());
        }

        sleep(Duration::from_secs(interval)).await;
        let poll_form = vec![
            (
                "grant_type".to_string(),
                "urn:ietf:params:oauth:grant-type:device_code".to_string(),
            ),
            ("device_code".to_string(), payload.device_code.clone()),
            ("client_id".to_string(), config.client_id.clone()),
        ];
        let response = client
            .post(&discovery.token_endpoint)
            .form(&poll_form)
            .send()
            .await
            .map_err(|err| format!("OAuth device token poll failed: {err}"))?;

        if response.status().is_success() {
            return parse_token_response(response).await;
        }

        let status = response.status();
        let err = response
            .json::<OAuthErrorResponse>()
            .await
            .unwrap_or_else(|_| OAuthErrorResponse {
                error: format!("http_{status}"),
                error_description: None,
            });
        match err.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => {
                interval = interval.saturating_add(5);
            }
            "access_denied" => {
                return Err("OAuth device flow was denied by user".to_string());
            }
            "expired_token" => {
                return Err("OAuth device code expired before completion".to_string());
            }
            other => {
                return Err(format!(
                    "OAuth device flow error: {}{}",
                    other,
                    err.error_description
                        .map(|desc| format!(" ({desc})"))
                        .unwrap_or_default()
                ));
            }
        }
    }
}

async fn run_auth_code_flow(
    client: &reqwest::Client,
    discovery: &OidcDiscovery,
    config: &ConnectOauthConfig,
) -> Result<TokenGrant, String> {
    let auth_endpoint = discovery.authorization_endpoint.as_ref().ok_or_else(|| {
        "Provider discovery did not include authorization_endpoint for auth-code flow".to_string()
    })?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("Failed to bind local OAuth callback listener: {err}"))?;
    let callback_addr = listener
        .local_addr()
        .map_err(|err| format!("Failed to read OAuth callback listener address: {err}"))?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", callback_addr.port());
    let state = Uuid::new_v4().to_string();

    let mut auth_url = Url::parse(auth_endpoint)
        .map_err(|err| format!("Invalid authorization endpoint: {err}"))?;
    {
        let scope = normalized_scope(config.scopes.clone());
        let mut query = auth_url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", &config.client_id);
        query.append_pair("redirect_uri", &redirect_uri);
        query.append_pair("state", &state);
        if !scope.is_empty() {
            query.append_pair("scope", &scope);
        }
        if let Some(audience) = config.audience.as_deref() {
            query.append_pair("audience", audience);
        }
    }

    println!("[mcpway] OAuth authorization required");
    println!("[mcpway] Open this URL: {}", auth_url.as_str());
    if !config.no_browser {
        let _ = try_open_browser(auth_url.as_str());
    }

    let (mut stream, _) = timeout(Duration::from_secs(300), listener.accept())
        .await
        .map_err(|_| "Timed out waiting for OAuth authorization callback".to_string())?
        .map_err(|err| format!("Failed to accept OAuth callback connection: {err}"))?;

    let mut buffer = [0u8; 4096];
    let size = stream
        .read(&mut buffer)
        .await
        .map_err(|err| format!("Failed to read OAuth callback request: {err}"))?;
    let request = String::from_utf8_lossy(&buffer[..size]).to_string();
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| "OAuth callback request was empty".to_string())?;
    let path = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "Invalid OAuth callback request line".to_string())?;
    let callback_url = Url::parse(&format!("http://127.0.0.1{path}"))
        .map_err(|err| format!("Invalid OAuth callback URL: {err}"))?;

    let mut code = None;
    let mut callback_state = None;
    let mut callback_error = None;
    for (key, value) in callback_url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => callback_state = Some(value.to_string()),
            "error" => callback_error = Some(value.to_string()),
            _ => {}
        }
    }

    let response_body = if let Some(ref err) = callback_error {
        format!("OAuth authorization failed: {err}. You can close this window.")
    } else {
        "OAuth authorization received. You can close this window.".to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response_body.len(),
        response_body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;

    if let Some(err) = callback_error {
        return Err(format!(
            "OAuth authorization callback returned error: {err}"
        ));
    }
    let callback_state = callback_state
        .ok_or_else(|| "OAuth authorization callback missing state parameter".to_string())?;
    if callback_state != state {
        return Err("OAuth authorization callback state mismatch".to_string());
    }
    let code =
        code.ok_or_else(|| "OAuth authorization callback missing code parameter".to_string())?;

    let form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("client_id".to_string(), config.client_id.clone()),
        ("code".to_string(), code),
        ("redirect_uri".to_string(), redirect_uri),
    ];
    let token_response = client
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|err| format!("OAuth authorization_code token request failed: {err}"))?;
    parse_token_response(token_response).await
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenGrant, String> {
    if response.status().is_success() {
        let payload = response
            .json::<TokenSuccessResponse>()
            .await
            .map_err(|err| format!("Invalid OAuth token response: {err}"))?;
        let expires_at = payload
            .expires_in
            .map(|seconds| unix_timestamp_secs().saturating_add(seconds));
        return Ok(TokenGrant {
            access_token: payload.access_token,
            refresh_token: payload.refresh_token,
            token_type: payload
                .token_type
                .filter(|token| !token.trim().is_empty())
                .unwrap_or_else(|| "Bearer".to_string()),
            expires_at_utc: expires_at,
            scope: payload.scope.unwrap_or_default(),
        });
    }

    let status = response.status();
    let err = response
        .json::<OAuthErrorResponse>()
        .await
        .unwrap_or_else(|_| OAuthErrorResponse {
            error: format!("http_{status}"),
            error_description: None,
        });
    Err(format!(
        "OAuth token request failed: {}{}",
        err.error,
        err.error_description
            .map(|desc| format!(" ({desc})"))
            .unwrap_or_default()
    ))
}

fn build_record(grant: &TokenGrant, config: &ConnectOauthConfig) -> TokenRecord {
    TokenRecord {
        access_token: grant.access_token.clone(),
        refresh_token: grant.refresh_token.clone(),
        token_type: grant.token_type.clone(),
        expires_at_utc: grant.expires_at_utc,
        obtained_at_utc: unix_timestamp_secs(),
        issuer: config.issuer.clone(),
        client_id: config.client_id.clone(),
        scope: if !grant.scope.trim().is_empty() {
            grant.scope.clone()
        } else {
            normalized_scope(config.scopes.clone())
        },
        audience: config.audience.clone(),
        profile: config.profile.clone(),
    }
}

fn normalized_scope(scopes: Vec<String>) -> String {
    let mut values = scopes
        .into_iter()
        .map(|scope| scope.trim().to_string())
        .filter(|scope| !scope.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values.join(" ")
}

fn profile_fingerprint(config: &ConnectOauthConfig) -> String {
    let payload = format!(
        "profile={}|issuer={}|client_id={}|scope={}|audience={}",
        config.profile.clone().unwrap_or_default(),
        config.issuer.trim(),
        config.client_id.trim(),
        normalized_scope(config.scopes.clone()),
        config.audience.clone().unwrap_or_default(),
    );
    sha256_hex(payload.as_bytes())
}

fn token_is_valid(record: &TokenRecord, now: u64) -> bool {
    if record.access_token.trim().is_empty() {
        return false;
    }
    match record.expires_at_utc {
        Some(expires) => expires.saturating_sub(60) > now,
        None => true,
    }
}

fn load_store(path: &Path) -> Result<OAuthCacheStore, String> {
    if !path.exists() {
        return Ok(OAuthCacheStore {
            schema_version: default_schema_version(),
            records: BTreeMap::new(),
        });
    }

    let body = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read OAuth cache {}: {err}", path.display()))?;
    match serde_json::from_str::<OAuthCacheStore>(&body) {
        Ok(mut store) => {
            if store.schema_version.trim().is_empty() {
                store.schema_version = default_schema_version();
            }
            Ok(store)
        }
        Err(err) => {
            let backup = path.with_extension(format!("corrupt-{}", unix_timestamp_secs()));
            let _ = std::fs::rename(path, &backup);
            eprintln!(
                "[mcpway] Warning: OAuth cache was invalid and has been rotated to {} ({err})",
                backup.display()
            );
            Ok(OAuthCacheStore {
                schema_version: default_schema_version(),
                records: BTreeMap::new(),
            })
        }
    }
}

fn save_store(path: &Path, store: &OAuthCacheStore) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid OAuth cache path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;

    let body = serde_json::to_string_pretty(store)
        .map_err(|err| format!("Failed to serialize OAuth cache: {err}"))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(|err| {
        format!(
            "Failed to write OAuth cache temp file {}: {err}",
            tmp.display()
        )
    })?;
    std::fs::rename(&tmp, path).map_err(|err| {
        format!(
            "Failed to atomically replace OAuth cache {}: {err}",
            path.display()
        )
    })
}

fn try_open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via open: {err}"))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via cmd /C start: {err}"))?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via xdg-open: {err}"))?;
        Ok(())
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    format!("{digest:x}")
}

fn default_schema_version() -> String {
    "1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_scope_sorts_and_deduplicates() {
        let scope = normalized_scope(vec![
            "mcp.write".to_string(),
            "mcp.read".to_string(),
            "mcp.read".to_string(),
        ]);
        assert_eq!(scope, "mcp.read mcp.write");
    }

    #[test]
    fn profile_fingerprint_is_stable() {
        let config = ConnectOauthConfig {
            profile: Some("demo".to_string()),
            issuer: "https://issuer.example.com".to_string(),
            client_id: "abc".to_string(),
            scopes: vec!["mcp.read".to_string()],
            flow: OAuthFlow::Device,
            no_browser: false,
            cache_path: None,
            login: false,
            logout: false,
            audience: Some("my-api".to_string()),
        };
        let a = profile_fingerprint(&config);
        let b = profile_fingerprint(&config);
        assert_eq!(a, b);
    }
}
