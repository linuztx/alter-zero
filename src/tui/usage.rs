//! The `/usage` page at the boundary: which subscriptions to read, and the
//! reads (`docs/usage.md`).
//!
//! The pure side owns the page and every word on it. What the boundary adds
//! is what `App` cannot know on its own — which subscriptions this machine
//! is signed in to (the providers table and the key store), the wall clock
//! and its offset — and the requests, one worker per subscription
//! (`workers::spawn_usage_fetch`), whose answers land through
//! [`Session::on_usage_result`] tagged with the fetch they belong to.

use alter_zero::app::ToastKind;
use alter_zero::stream::CancelToken;

use super::workers::{UsageFetch, spawn_usage_fetch};
use super::{Session, host};

/// What `/usage` says with nothing to read: the `/export` rule — a toast
/// naming the way in, never an empty page.
const USAGE_NONE_NOTICE: &str = "No subscription signed in — /login → Use a subscription";

/// The wall clock in Unix seconds, as the page's instants are kept.
fn now() -> i64 {
    i64::try_from(host::unix_secs()).unwrap_or(i64::MAX)
}

impl Session<'_> {
    /// `/usage`: open the page over every signed-in subscription and start
    /// reading each one — or, with none signed in, say so in a toast.
    pub(crate) fn open_usage_page(&mut self) {
        let subscriptions = self.models.usage_subscriptions();
        if subscriptions.is_empty() {
            self.toast(USAGE_NONE_NOTICE, ToastKind::Info);
            return;
        }
        let entries = subscriptions
            .iter()
            .map(|(entry, _)| entry.clone())
            .collect();
        let generation = self
            .app
            .open_usage_page(entries, now(), host::utc_offset_secs());
        self.spawn_usage_fetches(generation);
    }

    /// `r` on the page: read every subscription again, the rows keeping
    /// their numbers until the new answers land.
    pub(crate) fn refresh_usage(&mut self) {
        if let Some(generation) = self.app.begin_usage_refresh(now()) {
            self.spawn_usage_fetches(generation);
        }
    }

    /// The page closed: stop the reads still out, so a slow one reports
    /// nothing (the generation guard would drop it anyway).
    pub(crate) fn cancel_usage_fetch(&mut self) {
        if let Some(cancel) = self.usage_cancel.take() {
            cancel.cancel();
        }
    }

    /// One read per subscription on the page, under a fresh token — the
    /// previous fetch's cancelled first. The configs are resolved now, not
    /// at open, so a sign-in that changed meanwhile is the one read; a
    /// subscription no longer signed in lands as the sentence that says so.
    fn spawn_usage_fetches(&mut self, generation: u64) {
        self.cancel_usage_fetch();
        let cancel = CancelToken::new();
        self.usage_cancel = Some(cancel.clone());
        let mut configs = self.models.usage_subscriptions();
        let rows: Vec<(String, String)> = self
            .app
            .usage_page
            .as_ref()
            .map(|page| {
                page.entries
                    .iter()
                    .map(|entry| (entry.provider_id.clone(), entry.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (provider_id, name) in rows {
            match configs
                .iter()
                .position(|(entry, _)| entry.provider_id == provider_id)
            {
                Some(index) => {
                    let (_, cfg) = configs.swap_remove(index);
                    spawn_usage_fetch(
                        generation,
                        provider_id,
                        cfg,
                        cancel.clone(),
                        self.usage_tx.clone(),
                    );
                }
                None => {
                    self.app.set_usage_result(
                        generation,
                        &provider_id,
                        Err(format!("{name} is not signed in — /login signs in.")),
                        now(),
                    );
                }
            }
        }
    }

    /// A worker's answer: onto its row when it belongs to the page's current
    /// fetch, and a frame to show it.
    pub(crate) fn on_usage_result(&mut self, fetch: UsageFetch) {
        if self.app.set_usage_result(
            fetch.generation,
            &fetch.provider_id,
            fetch.result,
            fetch.fetched_at,
        ) {
            self.frame.schedule_frame();
        }
    }
}
