//! Misc coverage of the key-handler contract.

use super::*;

#[test]
fn backtab_without_support_raises_an_info_toast() {
    // A model with no reasoning (or the dummy backend): Shift+Tab explains
    // instead of dying silently. The *loop* presents the toast (arming its
    // expiry), so this is an Action, not a direct show_toast.
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    assert_eq!(
        app.on_key(backtab()),
        Action::Toast("dummy_model_name does not support thinking".into())
    );
    assert!(app.thinking.is_none());
}
