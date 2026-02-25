use std::sync::Arc;

type CleanupHandler = Arc<dyn Fn() + Send + Sync>;

pub fn install_signal_handlers(cleanup: Option<CleanupHandler>) {
    #[cfg(unix)]
    install_unix_signal_handlers(cleanup.clone());

    #[cfg(not(unix))]
    install_ctrl_c_handler(cleanup);
}

#[cfg(unix)]
fn install_unix_signal_handlers(cleanup: Option<CleanupHandler>) {
    use tokio::signal::unix::{signal, SignalKind};

    let handler = move |name: &'static str, cleanup: Option<CleanupHandler>| {
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

#[cfg(not(unix))]
fn install_ctrl_c_handler(cleanup: Option<CleanupHandler>) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("Caught CTRL+C. Exiting...");
            if let Some(cleanup) = cleanup {
                cleanup();
            }
            std::process::exit(0);
        }
    });
}
