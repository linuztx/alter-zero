//! The `/login` flow's boundary half: opening it with the choices the file
//! and the key store between them decide, running a subscription's device
//! sign-in on a worker, and persisting what it mints. See `docs/copilot.md`
//! and `docs/llm.md`.

use alter_zero::app::{SigninKind, ToastKind};
use alter_zero::clipboard;
use alter_zero::llm;
use alter_zero::stream::CancelToken;

use super::Session;
use super::workers::{DeviceEvent, spawn_signin};

/// The toast a copied device code raises — the `/copy` confirmation's sibling,
/// worded for what was actually copied.
const CODE_COPIED_NOTICE: &str = "Copied the code to clipboard";

impl Session<'_> {
    /// `/login` (`docs/llm.md`): open the inline onboarding, its two lists
    /// built from the provider file — the subscriptions signed in to, and the
    /// providers keyed — each row's ✓ reflecting real key resolution. The hint
    /// names the real `.env` path.
    pub(crate) fn open_key_onboarding(&mut self) {
        let providers = self.models.provider_choices();
        let subscriptions = self.models.subscription_choices();
        let env_path = self.models.env_path_display().to_string();
        self.app
            .open_key_onboarding(providers, subscriptions, env_path);
    }

    /// Enter on a subscription row (or its sign-in method choice): start the
    /// sign-in `kind` names on a worker thread. The pure core already opened
    /// the page; this only supplies it. A provider that isn't actually a
    /// sign-in one reports on the page rather than silently doing nothing —
    /// the row could only come from a provider file we misread.
    pub(crate) fn start_device_login(&mut self, provider: &str, kind: SigninKind) {
        self.cancel_device_login();
        if !self.models.is_subscription(provider) {
            self.app
                .fail_device_login(format!("{provider} has no subscription sign-in."));
            return;
        }
        let cancel = CancelToken::new();
        self.device_cancel = Some(cancel.clone());
        spawn_signin(provider.to_string(), kind, cancel, self.device_tx.clone());
    }

    /// Reap a running device flow's worker, if any. Idempotent — Esc, Ctrl+C
    /// and a completed flow all land here.
    pub(crate) fn cancel_device_login(&mut self) {
        if let Some(cancel) = self.device_cancel.take() {
            cancel.cancel();
        }
        self.device_expires = None;
    }

    /// `c` on the device page: copy the one-time code, and say so. The
    /// clipboard I/O is `/copy`'s (`docs/copy.md`); only the confirmation
    /// differs, because "Copied last message" would be a lie here.
    pub(crate) fn copy_device_code(&mut self, code: &str) {
        match clipboard::copy_to_clipboard(code) {
            Ok(lease) => {
                self.clipboard_lease = lease;
                self.toast(CODE_COPIED_NOTICE, ToastKind::Info);
            }
            Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
        }
    }

    /// The device-flow worker reported. A code goes on the page (and starts
    /// its countdown); a verdict either persists the token and closes the
    /// flow, or leaves the page up wearing the reason — the user must be able
    /// to read *why* before dismissing it.
    pub(crate) fn on_device_event(&mut self, event: DeviceEvent) {
        match event {
            DeviceEvent::Code {
                verification_uri,
                user_code,
                expires_at,
            } => {
                self.device_expires = Some(expires_at);
                self.app.set_device_code(&verification_uri, &user_code);
            }
            DeviceEvent::Done(Ok((token, plan))) => {
                self.finish_device_login(&token, plan.as_deref());
            }
            DeviceEvent::Done(Err(reason)) => {
                self.cancel_device_login();
                self.app.fail_device_login(reason);
            }
        }
    }

    /// A sign-in succeeded: persist the OAuth token under the provider's env
    /// var — the same `.env` store a pasted key lands in, so key resolution,
    /// the `/model` picker's ✓ and the next launch all pick it up with no
    /// second mechanism — then close the flow with a confirmation.
    fn finish_device_login(&mut self, token: &str, plan: Option<&str>) {
        self.cancel_device_login();
        let Some(provider) = self
            .app
            .key_onboarding
            .as_ref()
            .and_then(|o| o.device.as_ref())
            .map(|d| (d.provider_id.clone(), d.provider_name.clone()))
        else {
            // The page went away while the poll was in flight (a Ctrl+C, or a
            // `/login` reopened elsewhere): there is nothing to attach the
            // token to, and guessing a provider would key the wrong one.
            return;
        };
        let (id, name) = provider;
        let env_var = self.models.key_env(&id);
        // Signing in again with the same account is how a user recovers from a
        // credential the provider has stopped honouring, so drop the cached one
        // rather than handing the next request the exact token they just
        // re-authenticated to replace. Neither cache knows the other's keys,
        // so both are told — the miss is free.
        llm::copilot::forget(token);
        llm::chatgpt::forget(token);
        llm::claude::forget(token);
        match self.models.save_api_key(&env_var, token) {
            Ok(()) => {
                self.app.close_key_onboarding();
                // Name the seat when the exchange did: "Signed in ✓" leaves
                // open the one thing the user wants to know, and a metered
                // free seat's remaining quota is better said now than met as
                // a 402 mid-turn.
                let seat = plan.map_or_else(|| name.clone(), |p| format!("{name} ({p})"));
                self.toast(
                    format!("Signed in to {seat} — run /model to use it"),
                    ToastKind::Info,
                );
            }
            // The sign-in worked but the store didn't: say so on the page,
            // which is still the only place the failure makes sense.
            Err(reason) => self.app.fail_device_login(reason),
        }
    }
}
