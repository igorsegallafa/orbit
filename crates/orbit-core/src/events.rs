//! Events from long-running work (file watching, builds, agent runs) to the UI.
//! The app sends them down a channel.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn closures_receive_serialized_payloads() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Line<'a> {
            build_key: &'a str,
            n: u32,
        }
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let events = Events::new(move |event: &str, payload: Value| sink.lock().unwrap().push((event.to_string(), payload)));
        events.emit("build-output", Line { build_key: "k", n: 2 }).unwrap();
        events.clone().emit("plain", "text").unwrap();
        let got = got.lock().unwrap();
        assert_eq!(got[0], ("build-output".to_string(), serde_json::json!({ "buildKey": "k", "n": 2 })));
        assert_eq!(got[1].1, Value::String("text".into()));
    }
}
