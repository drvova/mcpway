use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

const MAX_LOG_FILE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLogRecord {
    pub ts_utc: u64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub mode: String,
    pub transport: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

pub struct LogFileLayer {
    writer: Arc<Mutex<BufWriter<File>>>,
    mode: String,
    transport: String,
}

pub fn default_log_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MCPWAY_LOG_PATH") {
        return PathBuf::from(path);
    }
    if let Some(home) = crate::discovery::user_home_dir() {
        return home
            .join(".mcpway")
            .join("logs")
            .join("mcpway.ndjson");
    }
    PathBuf::from(".mcpway/logs/mcpway.ndjson")
}

pub fn ensure_log_file(path: &Path) -> Result<(), String> {
    let _ = prepare_log_file(path)?;
    Ok(())
}

pub fn build_log_file_layer(mode: &str, transport: &str) -> Result<LogFileLayer, String> {
    let path = default_log_path();
    let file = prepare_log_file(&path)?;
    Ok(LogFileLayer {
        writer: Arc::new(Mutex::new(BufWriter::new(file))),
        mode: mode.to_string(),
        transport: transport.to_string(),
    })
}

pub fn parse_record(line: &str) -> Option<StoredLogRecord> {
    serde_json::from_str::<StoredLogRecord>(line).ok()
}

fn prepare_log_file(path: &Path) -> Result<File, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid log file path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;

    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_LOG_FILE_BYTES {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .map_err(|err| format!("Failed to truncate {}: {err}", path.display()))?;
        }
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| format!("Failed to open {}: {err}", path.display()))
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Default)]
struct EventVisitor {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(rendered.clone());
        }
        self.fields.insert(field.name().to_string(), rendered);
    }
}

impl<S> Layer<S> for LogFileLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        let metadata = event.metadata();
        let record = StoredLogRecord {
            ts_utc: unix_timestamp_secs(),
            level: metadata.level().as_str().to_ascii_lowercase(),
            target: metadata.target().to_string(),
            message: visitor.message.unwrap_or_default(),
            mode: self.mode.clone(),
            transport: self.transport.clone(),
            fields: visitor.fields,
        };

        let Ok(line) = serde_json::to_string(&record) else {
            return;
        };

        let Ok(mut writer) = self.writer.lock() else {
            return;
        };
        if writeln!(&mut *writer, "{line}").is_ok() {
            let _ = writer.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_record_roundtrip() {
        let line = serde_json::to_string(&StoredLogRecord {
            ts_utc: 123,
            level: "info".to_string(),
            target: "test".to_string(),
            message: "hello".to_string(),
            mode: "connect".to_string(),
            transport: "sse".to_string(),
            fields: BTreeMap::new(),
        })
        .expect("serialize");

        let parsed = parse_record(&line).expect("parse record");
        assert_eq!(parsed.level, "info");
        assert_eq!(parsed.transport, "sse");
    }
}
