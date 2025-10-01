use crate::app::App;
use std::sync::Arc;

use tokio::{
    sync::mpsc,
    time::{Duration, interval},
};

pub mod calendar;
pub mod email;

pub struct SyncDaemon {
    app: Arc<App>,
    email_rx: mpsc::Receiver<EmailSyncMessage>,
    calendar_rx: mpsc::Receiver<CalendarSyncMessage>,
}

#[derive(Debug)]
pub enum EmailSyncMessage {
    NewEmail(crate::Email),
    EmailRead(String), // message_id
    EmailDeleted(String),
}

#[derive(Debug)]
pub enum CalendarSyncMessage {
    NewEvent(crate::Event),
    EventUpdated(crate::Event),
    EventDeleted(i64),
}

impl SyncDaemon {
    pub fn new(app: Arc<App>) -> (Self, SyncHandles) {
        let (email_tx, email_rx) = mpsc::channel(1000);
        let (calendar_tx, calendar_rx) = mpsc::channel(1000);

        let daemon = Self {
            app,
            email_rx,
            calendar_rx,
        };

        let handles = SyncHandles {
            email_tx,
            calendar_tx,
        };

        (daemon, handles)
    }

    pub async fn run(&mut self) {
        let mut sync_interval = interval(Duration::from_secs(60)); // Sync every minute

        loop {
            tokio::select! {
                _ = sync_interval.tick() => {
                    // Periodic sync operations
                    self.periodic_sync().await;
                }
                Some(email_msg) = self.email_rx.recv() => {
                    self.handle_email_sync(email_msg).await;
                }
                Some(calendar_msg) = self.calendar_rx.recv() => {
                    self.handle_calendar_sync(calendar_msg).await;
                }
            }
        }
    }

    async fn periodic_sync(&self) {
        // Background sync logic - non-blocking
        println!("🔄 Running periodic sync...");
    }

    async fn handle_email_sync(&self, msg: EmailSyncMessage) {
        match msg {
            EmailSyncMessage::NewEmail(email) => {
                // Store email in local cache
                println!("📧 New email: {}", email.subject);
            }
            _ => {}
        }
    }

    async fn handle_calendar_sync(&self, msg: CalendarSyncMessage) {
        // Handle calendar updates
    }
}

pub struct SyncHandles {
    pub email_tx: mpsc::Sender<EmailSyncMessage>,
    pub calendar_tx: mpsc::Sender<CalendarSyncMessage>,
}
