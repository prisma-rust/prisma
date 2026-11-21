//! The app must not grab the pointer when the host owns the cursor.
//!
//! `PointerLockPolicy::Auto` (the FPS default) hides the cursor on the first left
//! click and re-grabs it whenever the window regains focus. An editor needs a
//! visible pointer for selecting and dragging, so it opts into `Manual`.

use prism_app::{App, PointerLockPolicy};

#[test]
fn the_default_policy_is_auto() {
    assert_eq!(PointerLockPolicy::default(), PointerLockPolicy::Auto);
}

#[test]
fn a_host_can_take_pointer_ownership_back() {
    let app = App::hosted(prism_engine::config::AppConfig::default())
        .with_pointer_lock_policy(PointerLockPolicy::Manual);
    assert_eq!(app.pointer_lock_policy(), PointerLockPolicy::Manual);
}

#[test]
fn hosted_mode_alone_does_not_change_the_policy() {
    // Hosted mode is about scenes, not input: an FPS-style host can still use it.
    let app = App::hosted(prism_engine::config::AppConfig::default());
    assert_eq!(app.pointer_lock_policy(), PointerLockPolicy::Auto);
}
