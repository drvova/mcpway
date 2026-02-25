use std::sync::Arc;

use tokio::signal::unix::{signal, SignalKind};

pub fn install_signal_handlers(cleanup: Option<Arc<dyn Fn() + Send + Sync>>) {
    let handler = move |name: &'static str, cleanup: Option<Arc<dyn Fn() + Send + Sync>>| {
        tokio::spawn(async move {
            if let Ok(mut sig) = signal(match name {
                "SIGINT" => SignalKind::interrupt(),
                "SIGTERM" => SignalKind::terminate(),
                _ => SignalKind::hangup(),
            }) {
                sig.recv().await;
                tracing::info!("Caught {name}. Exiting...");
                if let Some(cleanup) = cleanup {
                    cleanup();
                }
                std::process::exit(0);
            }
        });
    };

    handler("SIGINT", cleanup.clone());
    handler("SIGTERM", cleanup.clone());
    handler("SIGHUP", cleanup);
}
