use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::{
    AuthorizationStatus, NotificationActionTarget, NotificationProvider, NotificationRequest,
    PlatformStatus,
};

#[derive(Clone, Debug)]
pub struct MockNotificationProvider {
    state: Arc<Mutex<MockNotificationState>>,
}

#[derive(Clone, Debug)]
struct MockNotificationState {
    status: PlatformStatus,
    authorization: AuthorizationStatus,
    scheduled: Vec<NotificationRequest>,
    cancelled: Vec<String>,
    actions: Vec<NotificationActionTarget>,
}

impl Default for MockNotificationProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockNotificationProvider {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockNotificationState {
                status: PlatformStatus::Available,
                authorization: AuthorizationStatus::Authorized,
                scheduled: Vec::new(),
                cancelled: Vec::new(),
                actions: Vec::new(),
            })),
        }
    }

    pub fn with_status(self, status: PlatformStatus) -> Self {
        self.state.lock().unwrap().status = status;
        self
    }

    pub fn with_authorization(self, authorization: AuthorizationStatus) -> Self {
        self.state.lock().unwrap().authorization = authorization;
        self
    }

    pub fn push_action(&self, target: NotificationActionTarget) {
        self.state.lock().unwrap().actions.push(target);
    }

    pub fn scheduled(&self) -> Vec<NotificationRequest> {
        self.state.lock().unwrap().scheduled.clone()
    }

    pub fn cancelled(&self) -> Vec<String> {
        self.state.lock().unwrap().cancelled.clone()
    }
}

#[async_trait]
impl NotificationProvider for MockNotificationProvider {
    async fn schedule(&self, notifications: &[NotificationRequest]) -> anyhow::Result<Vec<String>> {
        let mut state = self.state.lock().unwrap();
        state.scheduled.extend_from_slice(notifications);
        Ok(notifications
            .iter()
            .map(|notification| notification.id.clone())
            .collect())
    }

    async fn cancel(&self, ids: &[String]) -> anyhow::Result<()> {
        self.state.lock().unwrap().cancelled.extend_from_slice(ids);
        Ok(())
    }

    async fn request_authorization(&self) -> anyhow::Result<AuthorizationStatus> {
        Ok(self.state.lock().unwrap().authorization)
    }

    async fn drain_action_targets(&self) -> Vec<NotificationActionTarget> {
        std::mem::take(&mut self.state.lock().unwrap().actions)
    }

    fn platform_status(&self) -> PlatformStatus {
        self.state.lock().unwrap().status
    }
}
