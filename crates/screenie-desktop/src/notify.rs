//! Desktop notifications (`org.freedesktop.Notifications`), with actions: how GNOME
//! apps tell of something done in the background and offer what to do next.

use std::collections::HashMap;

use futures_lite::StreamExt;
use screenie_core::APP_ID;
use zbus::zvariant::{Structure, Value};

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications",
    gen_blocking = false
)]
trait Server {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;
    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

/// A notification to show.
#[derive(Debug, Clone, Default)]
pub struct Notification {
    pub summary: String,
    pub body: String,
    /// A picture of what it's about: width, height and RGBA rows.
    pub image: Option<(u32, u32, Vec<u8>)>,
    /// Buttons: each action's key and label.
    pub actions: Vec<(String, String)>,
    /// What clicking the notification itself does, if anything.
    pub default_action: Option<String>,
}

/// What happened to a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// One of its actions was chosen.
    Action { id: u32, key: String },
    /// It's gone: dismissed, expired, or closed by us.
    Closed { id: u32 },
}

/// The notification server, and what it tells of the notifications shown.
pub struct Notifications {
    server: ServerProxy<'static>,
    /// Buttons are shown (some servers show none).
    actions: bool,
    events: async_channel::Receiver<Event>,
}

impl Notifications {
    /// Connect to the notification server. Fails where there's none.
    pub fn connect() -> zbus::Result<Notifications> {
        async_io::block_on(async {
            let connection = zbus::Connection::session().await?;
            let server = ServerProxy::builder(&connection)
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await?;
            let actions = server
                .get_capabilities()
                .await?
                .iter()
                .any(|c| c == "actions");
            let (tx, events) = async_channel::unbounded();
            let mut invoked = server.receive_action_invoked().await?;
            let mut closed = server.receive_notification_closed().await?;
            let sender = tx.clone();
            connection
                .executor()
                .spawn(
                    async move {
                        while let Some(signal) = invoked.next().await {
                            if let Ok(args) = signal.args() {
                                let event = Event::Action {
                                    id: args.id,
                                    key: args.action_key,
                                };
                                if sender.send(event).await.is_err() {
                                    break;
                                }
                            }
                        }
                    },
                    "notification actions",
                )
                .detach();
            connection
                .executor()
                .spawn(
                    async move {
                        while let Some(signal) = closed.next().await {
                            if let Ok(args) = signal.args()
                                && tx.send(Event::Closed { id: args.id }).await.is_err()
                            {
                                break;
                            }
                        }
                    },
                    "notifications closed",
                )
                .detach();
            Ok(Notifications {
                server,
                actions,
                events,
            })
        })
    }

    /// Whether the server shows action buttons.
    pub fn has_actions(&self) -> bool {
        self.actions
    }

    /// Show `notification`, in place of the one `replaces` if it's still up. Its id.
    pub fn show(&self, notification: &Notification, replaces: Option<u32>) -> zbus::Result<u32> {
        let mut actions: Vec<&str> = Vec::new();
        if let Some(key) = &notification.default_action {
            actions.extend([key.as_str(), ""]);
        }
        for (key, label) in &notification.actions {
            actions.extend([key.as_str(), label.as_str()]);
        }
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        // Named after the app, with its icon, and in its notification settings.
        hints.insert("desktop-entry", APP_ID.into());
        if let Some((width, height, rgba)) = &notification.image {
            let image = Structure::from((
                *width as i32,
                *height as i32,
                (*width * 4) as i32,
                true,
                8i32,
                4i32,
                rgba.clone(),
            ));
            hints.insert("image-data", image.into());
        }
        async_io::block_on(self.server.notify(
            "Screenie",
            replaces.unwrap_or(0),
            APP_ID,
            &notification.summary,
            &notification.body,
            &actions,
            hints,
            -1,
        ))
    }

    pub fn close(&self, id: u32) {
        let _ = async_io::block_on(self.server.close_notification(id));
    }

    /// What's happening to the notifications shown.
    pub fn events(&self) -> async_channel::Receiver<Event> {
        self.events.clone()
    }
}
