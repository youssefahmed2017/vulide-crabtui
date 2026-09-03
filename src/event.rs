//! Input and background messages funnel into one `AppEvent` stream.
//!
//! Phase 1 only sources terminal input. The background `mpsc` channel (for the
//! Phase 4 run console) plugs in here without touching the loop.

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event as CtEvent, KeyEvent, KeyEventKind};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),
    Resize(u16, u16),
    Tick,
}

/// Blocks up to `tick_rate` for a terminal event, yielding `Tick` on timeout.
pub struct EventSource {
    tick_rate: Duration,
}

impl EventSource {
    pub fn new(tick_rate: Duration) -> Self {
        Self { tick_rate }
    }

    pub fn next(&self) -> Result<Option<AppEvent>> {
        if !event::poll(self.tick_rate)? {
            return Ok(Some(AppEvent::Tick));
        }
        let ev = event::read()?;
        if std::env::var_os("VULIDE_EVLOG").is_some() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/vulide-events.log")
            {
                let _ = writeln!(f, "{ev:?}");
            }
        }
        Ok(match ev {
            CtEvent::Key(k) if k.kind == KeyEventKind::Press => Some(AppEvent::Key(k)),
            CtEvent::Key(_) => None,
            CtEvent::Paste(s) => Some(AppEvent::Paste(s)),
            CtEvent::Resize(w, h) => Some(AppEvent::Resize(w, h)),
            _ => None,
        })
    }
}
