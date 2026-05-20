use crate::actions::{drain_notification_action_targets, NotificationActionTarget};
use crate::platform_provider::{
    AuthorizationStatus, NotificationRequest, NotificationScheduler, PlatformStatus,
};
use async_trait::async_trait;

#[async_trait]
pub trait NotificationProvider: Send + Sync {
    async fn schedule(&self, notifications: &[NotificationRequest]) -> anyhow::Result<Vec<String>>;
    async fn cancel(&self, ids: &[String]) -> anyhow::Result<()>;
    async fn request_authorization(&self) -> anyhow::Result<AuthorizationStatus>;
    async fn drain_action_targets(&self) -> Vec<NotificationActionTarget>;
    fn platform_status(&self) -> PlatformStatus;
}

#[async_trait]
impl NotificationProvider for NotificationScheduler {
    async fn schedule(&self, notifications: &[NotificationRequest]) -> anyhow::Result<Vec<String>> {
        let mut scheduled = Vec::with_capacity(notifications.len());
        for notification in notifications {
            NotificationScheduler::schedule(self, notification)?;
            scheduled.push(notification.id.clone());
        }
        Ok(scheduled)
    }

    async fn cancel(&self, ids: &[String]) -> anyhow::Result<()> {
        NotificationScheduler::cancel(self, ids)?;
        Ok(())
    }

    async fn request_authorization(&self) -> anyhow::Result<AuthorizationStatus> {
        NotificationScheduler::request_authorization(self)?;
        Ok(NotificationScheduler::authorization_status(self)?)
    }

    async fn drain_action_targets(&self) -> Vec<NotificationActionTarget> {
        drain_notification_action_targets()
    }

    fn platform_status(&self) -> PlatformStatus {
        NotificationScheduler::platform_status(self)
    }
}
