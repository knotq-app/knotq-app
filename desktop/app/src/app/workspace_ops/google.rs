use gpui::Context;
use knotq_model::{
    CalendarProvider, GoogleOAuthAccount, ImportedCalendarSource, Scheme, SchemeSource,
};

use super::super::KnotQApp;

impl KnotQApp {
    pub(crate) fn imported_calendar_account_label(&self, scheme: &Scheme) -> Option<String> {
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            return None;
        };
        match source.provider {
            CalendarProvider::Google => source
                .account_email
                .clone()
                .or_else(|| {
                    self.google_calendar_local_account(source)
                        .and_then(|account| account.email.clone())
                })
                .or_else(|| Some(source.account_id.clone())),
            CalendarProvider::Apple | CalendarProvider::Ics => Some(source.account_id.clone()),
        }
        .filter(|label| !label.trim().is_empty())
    }

    pub(crate) fn google_calendar_has_local_credentials(&self, scheme: &Scheme) -> bool {
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            return true;
        };
        if source.provider != CalendarProvider::Google {
            return true;
        }
        self.google_calendar_local_account(source)
            .is_some_and(google_account_has_local_credentials)
    }

    pub(crate) fn google_calendar_local_account(
        &self,
        source: &ImportedCalendarSource,
    ) -> Option<&GoogleOAuthAccount> {
        self.settings
            .google_accounts
            .iter()
            .find(|account| google_account_matches_calendar_source(account, source))
    }

    pub(crate) fn google_calendar_scheme_count_for_account(
        &self,
        account: &GoogleOAuthAccount,
    ) -> usize {
        self.workspace
            .schemes
            .values()
            .filter(|scheme| {
                let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                    return false;
                };
                source.provider == CalendarProvider::Google
                    && google_account_matches_calendar_source(account, source)
            })
            .count()
    }

    pub(crate) fn unlink_google_account(&mut self, account_id: String, cx: &mut Context<Self>) {
        let old_len = self.settings.google_accounts.len();
        self.settings
            .google_accounts
            .retain(|account| account.account_id != account_id);
        if self.settings.google_accounts.len() != old_len {
            // Drop the account's OAuth tokens from the OS keychain too.
            let _ = knotq_storage_json::secrets::delete_google(&account_id);
            self.save_app_settings();
            cx.notify();
        }
    }
}

pub(crate) fn google_account_matches_calendar_source(
    account: &GoogleOAuthAccount,
    source: &ImportedCalendarSource,
) -> bool {
    if account.account_id == source.account_id {
        return true;
    }
    let Some(account_email) = account.email.as_deref() else {
        return false;
    };
    let source_email = source.account_email.as_deref().or_else(|| {
        source
            .account_id
            .contains('@')
            .then_some(source.account_id.as_str())
    });
    source_email.is_some_and(|source_email| emails_match(account_email, source_email))
}

pub(crate) fn google_account_has_local_credentials(account: &GoogleOAuthAccount) -> bool {
    !account.refresh_token.trim().is_empty()
}

pub(crate) fn google_calendar_source_target_label(source: &ImportedCalendarSource) -> String {
    source
        .account_email
        .clone()
        .filter(|email| !email.trim().is_empty())
        .unwrap_or_else(|| source.account_id.clone())
}

pub(crate) fn emails_match(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}
