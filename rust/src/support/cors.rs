use axum::http::header::HeaderName;
use regex::Regex;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::config::CorsConfig;

pub fn build_cors_layer(cors: &CorsConfig) -> Option<CorsLayer> {
    match cors {
        CorsConfig::Disabled => None,
        CorsConfig::AllowAll => Some(
            CorsLayer::very_permissive()
                .expose_headers([HeaderName::from_static("mcp-session-id")]),
        ),
        CorsConfig::AllowList { raw } => {
            if raw.is_empty() {
                return None;
            }
            if raw.iter().any(|origin| origin == "*") {
                return Some(
                    CorsLayer::very_permissive()
                        .expose_headers([HeaderName::from_static("mcp-session-id")]),
                );
            }
            let mut exact = Vec::new();
            let mut regexes: Vec<Regex> = Vec::new();
            for origin in raw {
                if origin.starts_with('/') && origin.ends_with('/') && origin.len() > 2 {
                    let pattern = &origin[1..origin.len() - 1];
                    if let Ok(re) = Regex::new(pattern) {
                        regexes.push(re);
                        continue;
                    }
                }
                exact.push(origin.clone());
            }
            let allow = AllowOrigin::predicate(move |origin, _req_parts| {
                if let Ok(origin_str) = origin.to_str() {
                    if exact.iter().any(|v| v == origin_str) {
                        return true;
                    }
                    for re in &regexes {
                        if re.is_match(origin_str) {
                            return true;
                        }
                    }
                }
                false
            });
            Some(
                CorsLayer::new()
                    .allow_origin(allow)
                    .expose_headers([HeaderName::from_static("mcp-session-id")]),
            )
        }
    }
}
