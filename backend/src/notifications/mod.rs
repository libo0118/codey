mod channels;
mod config;
mod dispatcher;
mod event;
mod formatting;
pub(crate) mod ilink;
pub(crate) mod outbox;

pub use config::{
    NotificationChannelConfig, NotificationChannelKind, NotificationChannelSessionStatus,
    WebhookConfig,
};
pub use dispatcher::NotificationDispatcher;
pub(crate) use dispatcher::shared_notification_http_client;
pub use event::NotificationEvent;
