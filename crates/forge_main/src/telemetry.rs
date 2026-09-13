//! Outbound Tracera telemetry wiring.
//!
//! Bridges [`forge_tracera`] into the `helioslite` / `forge` runtime so the
//! observability sink ships real lifecycle events instead of sitting dormant
//! as a self-contained, unit-tested library.
//!
//! Behavior:
//!
//! - **Opt-in and zero-cost by default.** Telemetry is disabled unless the
//!   `TRACERA_ENDPOINT` environment variable is set. When unset, `TraceraTelem`
//!   is a no-op and never allocates a runtime, a client, or a buffer.
//! - **Best-effort, never blocks shutdown.** `submit` is fire-and-forget;
//!   delivery happens on a background flush or on drop. A failed flush never
//!   fails the CLI.
//! - **Configurable via environment** — see [`TraceraTelem::from_env`].
use std::sync::Arc;

use forge_tracera::{AuthMode, EventKind, SinkConfig, TelemetrySink, TraceraEvent, TraceraSink};

/// Environment variable naming the Tracera collector endpoint.
pub const TRACERA_ENDPOINT_ENV: &str = "TRACERA_ENDPOINT";
/// Environment variable naming an optional bearer token.
pub const TRACERA_TOKEN_ENV: &str = "TRACERA_TOKEN";
/// Environment variable naming an optional HMAC secret.
pub const TRACERA_HMAC_SECRET_ENV: &str = "TRACERA_HMAC_SECRET";

/// Wrapper that lazily owns an optional [`TraceraSink`] and emits lifecycle
/// events into it.
///
/// Cheap to construct. Use [`TraceraTelem::from_env`] to populate it.
#[derive(Clone)]
pub struct TraceraTelem {
    inner: Option<Arc<TraceraSink>>,
}

impl TraceraTelem {
    /// Disabled telemetry. All methods are no-ops.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Build telemetry from environment variables.
    ///
    /// Disabled (returns `disabled()`) unless `TRACERA_ENDPOINT` is set. An
    /// invalid endpoint / auth is treated as *disabled* rather than fatal —
    /// telemetry must never take down the CLI.
    pub fn from_env() -> Self {
        let endpoint = match std::env::var(TRACERA_ENDPOINT_ENV) {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => return Self::disabled(),
        };

        let token = std::env::var(TRACERA_TOKEN_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty());
        let hmac_secret = std::env::var(TRACERA_HMAC_SECRET_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty());

        let auth = match (token, hmac_secret) {
            (_, Some(secret)) => {
                AuthMode::Hmac { secret, header: "X-Tracera-Signature".to_string() }
            }
            (Some(token), None) => AuthMode::Bearer { token },
            (None, None) => AuthMode::None,
        };

        let cfg = SinkConfig {
            endpoint,
            auth,
            source: "helioslite".to_string(),
            ..SinkConfig::default()
        };

        match TraceraSink::new(cfg) {
            Ok(sink) => Self { inner: Some(Arc::new(sink)) },
            Err(_) => Self::disabled(),
        }
    }

    /// True when telemetry is enabled.
    pub fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Submit an event. Silent on failure — telemetry is best-effort.
    pub async fn submit(&self, event: TraceraEvent) {
        if let Some(sink) = &self.inner {
            let _ = sink.submit(event).await;
        }
    }

    /// Emit a `session` lifecycle event (start / end / etc.).
    pub async fn session(&self, action: &str, detail: impl Into<serde_json::Value>) {
        if !self.enabled() {
            return;
        }
        let event = self.new_event(
            EventKind::Session,
            serde_json::json!({ "action": action, "detail": detail.into() }),
        );
        self.submit(event).await;
    }

    /// Emit a `custom` event for an arbitrary command dispatch.
    pub async fn command(&self, command: &str) {
        if !self.enabled() {
            return;
        }
        let event = self.new_event(
            EventKind::UserAction,
            serde_json::json!({ "command": command }),
        );
        self.submit(event).await;
    }

    /// Emit an error event.
    pub async fn error(&self, message: &str) {
        if !self.enabled() {
            return;
        }
        let event = self.new_event(
            EventKind::Custom,
            serde_json::json!({ "level": "error", "message": message }),
        );
        self.submit(event).await;
    }

    /// Force a delivery flush of any buffered events. Best-effort.
    pub async fn flush(&self) {
        if let Some(sink) = &self.inner {
            let _ = sink.flush().await;
        }
    }

    /// Shut down the sink, draining pending events. Best-effort.
    pub async fn shutdown(&self) {
        if let Some(sink) = &self.inner {
            let _ = sink.shutdown().await;
        }
    }

    fn new_event(&self, kind: EventKind, payload: serde_json::Value) -> TraceraEvent {
        TraceraEvent::new("helioslite", kind, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear_env() {
        unsafe {
            std::env::remove_var(TRACERA_ENDPOINT_ENV);
            std::env::remove_var(TRACERA_TOKEN_ENV);
            std::env::remove_var(TRACERA_HMAC_SECRET_ENV);
        }
    }

    #[test]
    #[serial]
    fn from_env_disabled_without_endpoint() {
        clear_env();
        let telem = TraceraTelem::from_env();
        assert!(!telem.enabled());
    }

    #[test]
    #[serial]
    fn from_env_enabled_with_endpoint_and_bearer() {
        clear_env();
        unsafe {
            std::env::set_var(TRACERA_ENDPOINT_ENV, "http://127.0.0.1:9999/v1/events");
            std::env::set_var(TRACERA_TOKEN_ENV, "sekrit");
        }
        let telem = TraceraTelem::from_env();
        assert!(telem.enabled());
    }

    #[test]
    #[serial]
    fn from_env_enabled_with_endpoint_and_hmac() {
        clear_env();
        unsafe {
            std::env::set_var(TRACERA_ENDPOINT_ENV, "http://127.0.0.1:9999/v1/events");
            std::env::set_var(TRACERA_HMAC_SECRET_ENV, "shhh");
        }
        let telem = TraceraTelem::from_env();
        assert!(telem.enabled());
    }
    #[test]
    #[serial]
    fn from_env_invalid_scheme_degrades_to_disabled() {
        clear_env();
        // ftp:// is not accepted by SinkConfig::validate, so construction is
        // rejected and telemetry degrades to disabled rather than panicking.
        unsafe {
            std::env::set_var(TRACERA_ENDPOINT_ENV, "ftp://nope/events");
        }
        let telem = TraceraTelem::from_env();
        assert!(!telem.enabled());
    }

    #[test]
    #[serial]
    fn from_env_blank_endpoint_degrades_to_disabled() {
        clear_env();
        unsafe {
            std::env::set_var(TRACERA_ENDPOINT_ENV, "   ");
        }
        let telem = TraceraTelem::from_env();
        assert!(!telem.enabled());
    }
}
