//! Event fan-out for `GET /api/events` (the SSE counterpart of Tauri's `listen`).

use std::sync::Arc;

use app_lib::events::EventEmitter;
use serde_json::Value;
use tokio::sync::broadcast;

/// One event exactly as the UI receives it: the frozen name (`job://progress`, …) and the
/// payload the control plane serialized.
pub type BroadcastEvent = (String, Value);

/// An [`EventEmitter`] that fans every event out to the connected SSE clients.
///
/// A slow or absent client never blocks the control plane: `send` drops the message when
/// nobody is subscribed, and a lagging client sees `Lagged` and skips to the newest event.
#[derive(Clone)]
pub struct BroadcastEmitter {
    sender: broadcast::Sender<BroadcastEvent>,
}

impl BroadcastEmitter {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// A new subscriber for the SSE stream.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEvent> {
        self.sender.subscribe()
    }
}

impl EventEmitter for BroadcastEmitter {
    fn emit(&self, event: &str, payload: Value) {
        // No subscriber means nobody is looking at the UI; the event is not stored.
        let _ = self.sender.send((event.to_string(), payload));
    }
}

/// Wrap the emitter so the state owns it as the trait object the control plane expects.
pub fn shared_emitter(capacity: usize) -> (Arc<BroadcastEmitter>, Arc<dyn EventEmitter>) {
    let emitter = Arc::new(BroadcastEmitter::new(capacity));
    let trait_object: Arc<dyn EventEmitter> = emitter.clone();
    (emitter, trait_object)
}
