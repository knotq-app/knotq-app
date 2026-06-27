//! WebSocket sync client lifecycle on `KnotQApp`: create it for a sync-enabled
//! account, keep its token holder fresh, and tear it down on sign-out / account
//! switch. The actual socket creation is behind the `ws-sync` feature, so without
//! that feature these are cheap no-ops and `ws_sync` stays `None` (HTTP-only).
use crate::app::KnotQApp;

impl KnotQApp {
    /// Ensure a WebSocket sync client matches the current account. Updates the
    /// shared token holder every call (so reconnects use a fresh token), and under
    /// `ws-sync` creates the client for a sync-enabled account, rebuilding it if the
    /// account's api_base changed. Called at the start of each sync run.
    pub(crate) fn ensure_ws_sync(&mut self) {
        let Some(account) = self.settings.sync_account.clone() else {
            self.teardown_ws_sync();
            return;
        };
        if !account.supports_sync {
            self.teardown_ws_sync();
            return;
        }
        // Keep the token holder fresh for the supervisor's reconnect handshakes.
        if let Ok(mut token) = self.ws_sync_token.lock() {
            *token = account.bearer_token.clone();
        }

        #[cfg(feature = "ws-sync")]
        {
            // Rebuild on an account/server switch so the socket targets the right host.
            if self.ws_sync_api_base.as_deref() != Some(account.api_base.as_str()) {
                self.teardown_ws_sync();
            }
            if self.ws_sync.is_none() {
                let token_holder = std::sync::Arc::clone(&self.ws_sync_token);
                let token_provider: super::ws_socket::TokenProvider =
                    std::sync::Arc::new(move || {
                        token_holder
                            .lock()
                            .ok()
                            .map(|token| token.clone())
                            .filter(|token| !token.is_empty())
                    });
                let sync_tx = self.service_bus.sync_signal_sender();
                let presence_tx = self.presence_tx.clone();
                let callbacks = knotq_sync::ws::WsCallbacks {
                    // A peer pushed: request an immediate sync run (over the socket).
                    on_changed: Box::new(move || {
                        let _ = sync_tx.try_send(crate::app::sync_service::SyncSignal::Immediate);
                    }),
                    // A peer's live caret: funnel to the GPUI thread to render.
                    on_presence: Box::new(move |event| {
                        let _ = presence_tx.try_send(event);
                    }),
                };
                let client = super::ws_socket::connect_workspace_ws(
                    &account.api_base,
                    token_provider,
                    knotq_sync::ws::WsConfig::default(),
                    callbacks,
                );
                self.ws_sync = Some(std::sync::Arc::new(client));
                self.ws_sync_api_base = Some(account.api_base.clone());
            }
        }
    }

    /// Stop and drop the WebSocket sync client (sign-out / account switch).
    pub(crate) fn teardown_ws_sync(&mut self) {
        if let Some(client) = self.ws_sync.take() {
            client.shutdown();
        }
        self.ws_sync_api_base = None;
    }
}
