use std::collections::HashMap;

pub type HeadersMap = HashMap<String, String>;

#[derive(Debug, Clone, Default)]
pub struct RuntimeArgs {
    pub extra_cli_args: Vec<String>,
    pub env: HashMap<String, String>,
    pub headers: HeadersMap,
}

impl RuntimeArgs {
    pub fn merge(base: &RuntimeArgs, overlay: &RuntimeArgs) -> RuntimeArgs {
        let mut merged = base.clone();
        if !overlay.extra_cli_args.is_empty() {
            merged
                .extra_cli_args
                .extend(overlay.extra_cli_args.iter().cloned());
        }
        if !overlay.env.is_empty() {
            for (k, v) in &overlay.env {
                merged.env.insert(k.clone(), v.clone());
            }
        }
        if !overlay.headers.is_empty() {
            for (k, v) in &overlay.headers {
                merged.headers.insert(k.clone(), v.clone());
            }
        }
        merged
    }
}
