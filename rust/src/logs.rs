use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::sleep;

use crate::config::{LogsConfig, LogsLevel, LogsTailConfig};
use crate::support::log_store::{default_log_path, ensure_log_file, parse_record, StoredLogRecord};

pub async fn run(config: LogsConfig) -> Result<(), String> {
    match config {
        LogsConfig::Tail(tail) => run_tail(tail).await,
    }
}

async fn run_tail(config: LogsTailConfig) -> Result<(), String> {
    let path = config.file.clone().unwrap_or_else(default_log_path);
    ensure_log_file(&path)?;

    print_recent_lines(&path, &config)?;
    if !config.follow {
        return Ok(());
    }

    let mut offset = file_len(&path).unwrap_or(0);
    loop {
        sleep(Duration::from_millis(500)).await;
        let current_len = file_len(&path).unwrap_or(0);
        if current_len < offset {
            offset = 0;
        }

        let mut file = File::open(&path)
            .map_err(|err| format!("Failed to open log file {}: {err}", path.display()))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|err| format!("Failed to seek {}: {err}", path.display()))?;

        let mut buffer = String::new();
        file.read_to_string(&mut buffer)
            .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
        offset = file
            .seek(SeekFrom::End(0))
            .map_err(|err| format!("Failed to seek end {}: {err}", path.display()))?;

        for line in buffer.lines() {
            emit_line(line, &config);
        }
    }
}

fn print_recent_lines(path: &Path, config: &LogsTailConfig) -> Result<(), String> {
    let body = std::fs::read_to_string(path)
        .map_err(|err| format!("Failed to read log file {}: {err}", path.display()))?;
    let mut window: VecDeque<&str> = VecDeque::new();
    for line in body.lines() {
        if window.len() >= config.lines {
            window.pop_front();
        }
        window.push_back(line);
    }

    for line in window {
        emit_line(line, config);
    }
    Ok(())
}

fn emit_line(line: &str, config: &LogsTailConfig) {
    let Some(record) = parse_record(line) else {
        return;
    };
    if !matches_filters(&record, config) {
        return;
    }

    if config.json {
        println!("{line}");
    } else {
        println!(
            "[{}][{}][{}:{}] {}",
            record.ts_utc,
            record.level.to_uppercase(),
            record.mode,
            record.transport,
            record.message
        );
    }
}

fn matches_filters(record: &StoredLogRecord, config: &LogsTailConfig) -> bool {
    if let Some(level) = config.level {
        let wanted = match level {
            LogsLevel::Debug => "debug",
            LogsLevel::Info => "info",
            LogsLevel::Warn => "warn",
            LogsLevel::Error => "error",
        };
        if record.level != wanted {
            return false;
        }
    }

    if let Some(transport) = config.transport {
        if record.transport != transport.as_str() {
            return false;
        }
    }

    true
}

fn file_len(path: &PathBuf) -> Option<u64> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .ok()
        .and_then(|file| file.metadata().ok())
        .map(|meta| meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogsTransport;

    #[test]
    fn filter_matches_expected_transport() {
        let record = StoredLogRecord {
            ts_utc: 1,
            level: "info".to_string(),
            target: "test".to_string(),
            message: "hello".to_string(),
            mode: "connect".to_string(),
            transport: "ws".to_string(),
            fields: Default::default(),
        };
        let cfg = LogsTailConfig {
            file: None,
            follow: false,
            lines: 10,
            level: None,
            transport: Some(LogsTransport::Ws),
            json: false,
        };
        assert!(matches_filters(&record, &cfg));
    }
}
