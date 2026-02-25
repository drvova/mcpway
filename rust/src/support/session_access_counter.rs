use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

enum SessionState {
    Active { count: usize },
    Timeout { handle: JoinHandle<()> },
}

pub struct SessionAccessCounter {
    timeout: Duration,
    cleanup: Arc<dyn Fn(String) + Send + Sync>,
    sessions: Mutex<HashMap<String, SessionState>>,
}

impl SessionAccessCounter {
    pub fn new(timeout_ms: u64, cleanup: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        Self {
            timeout: Duration::from_millis(timeout_ms),
            cleanup,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn inc(&self, session_id: &str, reason: &str) {
        tracing::info!("SessionAccessCounter.inc() {session_id}, caused by {reason}");
        let mut sessions = self.sessions.lock().await;
        match sessions.remove(session_id) {
            None => {
                tracing::info!("Session access count 0 -> 1 for {session_id} (new session)");
                sessions.insert(session_id.to_string(), SessionState::Active { count: 1 });
            }
            Some(SessionState::Timeout { handle }) => {
                handle.abort();
                tracing::info!(
                    "Session access count 0 -> 1, clearing cleanup timeout for {session_id}"
                );
                sessions.insert(session_id.to_string(), SessionState::Active { count: 1 });
            }
            Some(SessionState::Active { count }) => {
                tracing::info!(
                    "Session access count {count} -> {} for {session_id}",
                    count + 1
                );
                sessions.insert(
                    session_id.to_string(),
                    SessionState::Active { count: count + 1 },
                );
            }
        }
    }

    pub async fn dec(&self, session_id: &str, reason: &str) {
        tracing::info!("SessionAccessCounter.dec() {session_id}, caused by {reason}");
        let mut sessions = self.sessions.lock().await;
        let Some(state) = sessions.remove(session_id) else {
            tracing::error!("Called dec() on non-existent session {session_id}, ignoring");
            return;
        };
        match state {
            SessionState::Timeout { .. } => {
                tracing::error!(
                    "Called dec() on session {session_id} that is already pending cleanup, ignoring"
                );
                sessions.insert(session_id.to_string(), state);
            }
            SessionState::Active { count } => {
                if count == 0 {
                    tracing::error!("Invalid access count 0 for session {session_id}");
                    return;
                }
                let next = count - 1;
                tracing::info!("Session access count {count} -> {next} for {session_id}");
                if next == 0 {
                    tracing::info!(
                        "Session access count reached 0, setting cleanup timeout for {session_id}"
                    );
                    let timeout = self.timeout;
                    let session = session_id.to_string();
                    let cleanup = self.cleanup.clone();
                    let handle = tokio::spawn(async move {
                        tokio::time::sleep(timeout).await;
                        tracing::info!("Session {session} timed out, cleaning up");
                        cleanup(session);
                    });
                    sessions.insert(session_id.to_string(), SessionState::Timeout { handle });
                } else {
                    sessions.insert(session_id.to_string(), SessionState::Active { count: next });
                }
            }
        }
    }
}
