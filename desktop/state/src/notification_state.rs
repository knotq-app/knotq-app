use knotq_notifications::{NotificationProvider, NotificationRequest};

#[derive(Clone, Debug, Default)]
pub struct NotificationState {
    pub scheduled_ids: Vec<String>,
    pub pending_action_drains: usize,
}

pub async fn reschedule_notifications(
    state: &mut NotificationState,
    provider: &dyn NotificationProvider,
    requests: &[NotificationRequest],
) -> anyhow::Result<()> {
    let ids = provider.schedule(requests).await?;
    state.scheduled_ids = ids;
    Ok(())
}
