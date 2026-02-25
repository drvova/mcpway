use std::env;

use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::config::{LogLevel, OutputTransport};
use crate::support::log_store::build_log_file_layer;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

pub fn init_telemetry(
    log_level: LogLevel,
    output_transport: OutputTransport,
    mode: &str,
    transport: &str,
) -> TelemetryGuard {
    let default_filter = match log_level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::None => "off",
    };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let writer = if matches!(output_transport, OutputTransport::Stdio) {
        BoxMakeWriter::new(std::io::stderr)
    } else {
        BoxMakeWriter::new(std::io::stdout)
    };

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_writer(writer);
    let file_layer = build_log_file_layer(mode, transport)
        .map(Some)
        .unwrap_or_else(|err| {
            eprintln!("[mcpway] Failed to initialize file log sink: {err}");
            None
        });

    let mut tracer_provider = None;
    let mut logger_provider = None;
    let mut otel_trace_layer = None;
    let mut otel_log_layer = None;

    let base_endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let trace_endpoint = env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| base_endpoint.clone());
    let log_endpoint = env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| base_endpoint.clone());

    if trace_endpoint.is_some() || log_endpoint.is_some() {
        let resource = Resource::builder()
            .with_attributes([
                KeyValue::new("service.name", "mcpway"),
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            ])
            .build();

        if let Some(endpoint) = trace_endpoint {
            match SpanExporter::builder()
                .with_http()
                .with_endpoint(endpoint)
                .build()
            {
                Ok(exporter) => {
                    let provider = SdkTracerProvider::builder()
                        .with_batch_exporter(exporter)
                        .with_resource(resource.clone())
                        .build();
                    let tracer = provider.tracer("mcpway");
                    otel_trace_layer = Some(tracing_opentelemetry::layer().with_tracer(tracer));
                    global::set_tracer_provider(provider.clone());
                    tracer_provider = Some(provider);
                }
                Err(err) => {
                    eprintln!("[mcpway] Failed to init OTLP traces: {err}");
                }
            }
        }

        if let Some(endpoint) = log_endpoint {
            match LogExporter::builder()
                .with_http()
                .with_endpoint(endpoint)
                .build()
            {
                Ok(exporter) => {
                    let provider = SdkLoggerProvider::builder()
                        .with_resource(resource)
                        .with_log_processor(BatchLogProcessor::builder(exporter).build())
                        .build();
                    otel_log_layer = Some(OpenTelemetryTracingBridge::new(&provider));
                    logger_provider = Some(provider);
                }
                Err(err) => {
                    eprintln!("[mcpway] Failed to init OTLP logs: {err}");
                }
            }
        }
    }

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(file_layer)
        .with(otel_trace_layer)
        .with(otel_log_layer);

    if let Err(err) = registry.try_init() {
        eprintln!("[mcpway] Failed to init tracing subscriber: {err}");
    }

    TelemetryGuard {
        tracer_provider,
        logger_provider,
    }
}
