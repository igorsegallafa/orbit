//! Events from long-running work (file watching, builds, agent runs) to the UI.
//! The Tauri app forwards them to the webview with `AppHandle::emit`; the
//! native app sends them down a channel.
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
}

/// A cloneable handle on the app's sink.
#[derive(Clone)]
pub struct Events(Arc<dyn EventSink>);

impl Events {
    pub fn new(sink: impl EventSink) -> Self {
        Self(Arc::new(sink))
    }

    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> Result<(), String> {
        let value = serde_json::to_value(payload).map_err(|e| e.to_string())?;
        self.0.emit(event, value);
        Ok(())
    }
}

/// Drops every event (tests, and callers that don't listen).
pub struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: &str, _: Value) {}
}

impl<F: Fn(&str, Value) + Send + Sync + 'static> EventSink for F {
    fn emit(&self, event: &str, payload: Value) {
        self(event, payload)
    }
}
