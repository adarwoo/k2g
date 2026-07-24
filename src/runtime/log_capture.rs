//! In-memory capture of the application's `tracing`/`log` output so the UI can
//! show a live log viewer.
//!
//! The app already formats events to stdout through a `tracing_subscriber::fmt`
//! layer. This module adds a second, parallel [`CaptureLayer`] that records each
//! event into a bounded ring buffer ([`LOG_CAPACITY`] most-recent entries). The
//! Logs screen reads a snapshot of that buffer via [`snapshot`]; nothing here
//! touches the UI directly, keeping the capture usable from any thread (the
//! generation worker logs too).

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use chrono::Local;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// How many of the most-recent log lines are retained. Older lines are dropped
/// once the buffer is full — this is a live tail, not a persistent log file.
pub const LOG_CAPACITY: usize = 2000;

/// One captured log event, flattened for display.
#[derive(Clone, PartialEq)]
pub struct LogEntry {
    /// Local wall-clock time the event was recorded (`HH:MM:SS.mmm`).
    pub timestamp: String,
    /// Severity as an uppercase word (`ERROR`/`WARN`/`INFO`/`DEBUG`/`TRACE`).
    pub level: &'static str,
    /// The event's target — usually the emitting module path.
    pub target: String,
    /// The rendered log message.
    pub message: String,
}

fn buffer() -> &'static Mutex<VecDeque<LogEntry>> {
    static LOG_BUFFER: OnceLock<Mutex<VecDeque<LogEntry>>> = OnceLock::new();
    LOG_BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(LOG_CAPACITY)))
}

/// Append one entry, evicting the oldest when at capacity. A poisoned lock is
/// ignored — losing a log line must never take down a logging call site.
fn push(entry: LogEntry) {
    if let Ok(mut buf) = buffer().lock() {
        if buf.len() >= LOG_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

/// A chronological snapshot (oldest first) of the currently-retained log lines.
pub fn snapshot() -> Vec<LogEntry> {
    buffer()
        .lock()
        .map(|buf| buf.iter().cloned().collect())
        .unwrap_or_default()
}

/// Drop every retained log line (the Logs screen's "Clear" action).
pub fn clear() {
    if let Ok(mut buf) = buffer().lock() {
        buf.clear();
    }
}

/// Pulls the `message` field out of an event's fields. tracing records the log
/// message under the reserved field name `message`; structured key/value fields
/// are appended so they are not silently lost.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn note_field(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = rendered;
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            self.fields.push_str(&format!("{}={}", field.name(), rendered));
        }
    }

    /// The message with any structured fields appended in parentheses.
    fn into_message(self) -> String {
        match (self.message.is_empty(), self.fields.is_empty()) {
            (true, true) => String::new(),
            (false, true) => self.message,
            (true, false) => self.fields,
            (false, false) => format!("{} ({})", self.message, self.fields),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.note_field(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // The `message` field is `std::fmt::Arguments`, whose Debug renders the
        // already-formatted text (no surrounding quotes).
        self.note_field(field, format!("{value:?}"));
    }
}

/// The `tracing` layer that records events into the ring buffer. Add it to the
/// subscriber alongside the stdout `fmt` layer; the shared `EnvFilter` on the
/// registry gates both, so the viewer honours `RUST_LOG`.
pub struct CaptureLayer;

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        push(LogEntry {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            level: meta.level().as_str(),
            target: meta.target().to_string(),
            message: visitor.into_message(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_pushes_in_order_and_is_bounded() {
        clear();
        for i in 0..(LOG_CAPACITY + 5) {
            push(LogEntry {
                timestamp: "00:00:00.000".to_string(),
                level: "INFO",
                target: "test".to_string(),
                message: format!("line {i}"),
            });
        }
        let snap = snapshot();
        assert_eq!(snap.len(), LOG_CAPACITY, "buffer is capped at capacity");
        // Oldest lines were evicted; the tail is the most recent.
        assert_eq!(snap.first().unwrap().message, "line 5");
        assert_eq!(snap.last().unwrap().message, format!("line {}", LOG_CAPACITY + 4));
        clear();
        assert!(snapshot().is_empty(), "clear empties the buffer");
    }
}
