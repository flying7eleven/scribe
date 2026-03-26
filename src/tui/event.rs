use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use tokio::sync::mpsc;

/// Events produced by the TUI event loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A key press event.
    Key(KeyEvent),
    /// A periodic tick (used for Live tab polling).
    Tick,
    /// Terminal was resized.
    #[allow(dead_code)] // fields used by later stories for responsive layout
    Resize(u16, u16),
}

/// Reads crossterm events and tick intervals, sending them through a channel.
pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl EventHandler {
    /// Spawn the event handler. Reads crossterm events and a tick timer concurrently.
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick = tokio::time::interval(tick_rate);

            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if tx.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    }
                    event = reader.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                // Only handle key press events (not release/repeat)
                                if key.kind == KeyEventKind::Press
                                    && tx.send(AppEvent::Key(key)).is_err()
                                {
                                    break;
                                }
                            }
                            Some(Ok(Event::Resize(w, h))) => {
                                if tx.send(AppEvent::Resize(w, h)).is_err() {
                                    break;
                                }
                            }
                            Some(Ok(_)) => {} // ignore mouse, focus, paste events
                            Some(Err(_)) => break,
                            None => break,
                        }
                    }
                }
            }
        });

        Self { rx }
    }

    /// Wait for the next event.
    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }
}
