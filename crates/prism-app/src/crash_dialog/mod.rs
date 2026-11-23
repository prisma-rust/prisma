//! Native fatal error crash dialog
//!
//! When renderer encounters unrecoverable error (device lost, validation failed,
//! unable to recreate swapchain etc), we want to present error info to user once and stop,
//! instead of logging every frame. This module shows a **native modal dialog**, offering two operations:
//!
//! - **Copy and exit** (simultaneously copy full error text to clipboard, convenient for user to paste into error report)
//! - **Exit**
//!
//! Each platform uses its native dialog API, avoiding introducing heavyweight cross-platform dialog crate:
//!
//! | Platform | Dialog | Clipboard |
//! |----------|--------|-----------|
//! | Windows | `MessageBoxW` (`MB_YESNO`) | `OpenClipboard` / `SetClipboardData` |
//! | macOS    | `osascript` (`display dialog`) | `pbcopy` |
//! | Linux    | `zenity --question` (fallback: text on stderr) | `xclip`/`xsel` |
//! | Android | `AlertDialog` via JNI (modal polling wait) | `ClipboardManager` via JNI |
//!
//! The dialog blocks the calling thread (the winit event-loop / main thread)
//! Until user confirms, this naturally "pauses" render loop.
//! confirmation the caller tears down the event loop.
//!
//! ## Android
//!
//! The error text is logged to logcat (tag `Prisma`), an `AlertDialog` is
//! shown on UI thread via JNI against the `Activity` exposed by
//! [`android_activity::AndroidApp`], and calling thread waits modally until
//! the dialog is dismissed. The error text is also copied to Android system
//! clipboard via `ClipboardManager` on dismissal.

/// The user's choice in the crash dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashChoice {
    /// "Copy & Exit": copy the error text to the clipboard, then exit.
    CopyAndExit,
    /// "Exit": just exit.
    Exit,
}

#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy)]
struct AndroidAppHandles {
    vm_ptr: *mut std::ffi::c_void,
    activity_ptr: *mut std::ffi::c_void,
}

#[cfg(any(target_os = "android", test))]
unsafe impl Send for AndroidAppHandles {}
#[cfg(any(target_os = "android", test))]
unsafe impl Sync for AndroidAppHandles {}

#[cfg(any(target_os = "android", test))]
static ANDROID_APP: std::sync::RwLock<Option<AndroidAppHandles>> = std::sync::RwLock::new(None);

/// Register the `AndroidApp` so the crash dialog can reach the JVM/Activity
/// for showing an `AlertDialog`. Called from `android_main` before the
/// Event loop starts. Overwrites any previous registration so re-entering the
/// Android activity always replaces any stale Activity handle.
///
/// No-op on non-Android platforms (the function simply isn't compiled there).
#[cfg(target_os = "android")]
pub fn register_android_app(app: &android_activity::AndroidApp) {
    let handles = AndroidAppHandles {
        vm_ptr: app.vm_as_ptr(),
        activity_ptr: app.activity_as_ptr(),
    };
    if let Ok(mut guard) = ANDROID_APP.write() {
        *guard = Some(handles);
    }
}

/// Unregister the `AndroidApp` on activity exit to avoid retaining dangling
/// pointers to a destroyed Activity.
#[cfg(any(target_os = "android", test))]
pub fn unregister_android_app() {
    if let Ok(mut guard) = ANDROID_APP.write() {
        *guard = None;
    }
}

/// Show the native crash dialog with `title` and `message`.
///
/// Blocks the calling thread until the user confirms. Returns the chosen
/// action. If showing the dialog fails on a platform that uses a subprocess
/// (macOS/Linux) or JNI (Android), the error text is logged and `Exit` is
/// returned so the caller still terminates.
pub fn show_crash_dialog(title: &str, message: &str) -> CrashChoice {
    log::error!("FATAL: {title}\n{message}");

    let choice = show_native(title, message);
    if matches!(choice, CrashChoice::CopyAndExit) {
        if let Err(e) = copy_to_clipboard(message) {
            log::warn!("failed to copy crash text to clipboard: {e}");
        }
    }
    choice
}

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;

#[cfg(all(unix, not(target_os = "android"), not(target_os = "macos")))]
#[path = "linux.rs"]
mod platform;

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod platform;

#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "android",
    all(unix, not(target_os = "android"), not(target_os = "macos"))
)))]
#[path = "fallback.rs"]
mod platform;

fn show_native(title: &str, message: &str) -> CrashChoice {
    platform::show(title, message)
}

/// Copy `text` to system clipboard. Platform-specific.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    platform::copy_to_clipboard(text)
}

#[cfg(test)]
#[path = "android.rs"]
mod test_android_syntax;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_clipboard_noop_when_not_registered() {
        let res = test_android_syntax::copy_to_clipboard("test message");
        assert!(res.is_ok());
    }

    #[test]
    fn android_show_exits_when_not_registered() {
        let choice = test_android_syntax::show("title", "message");
        assert_eq!(choice, CrashChoice::Exit);
    }

    #[test]
    fn android_app_registration_is_reentrant() {
        let ptr1 = 0x1234 as *mut std::ffi::c_void;
        let ptr2 = 0x5678 as *mut std::ffi::c_void;

        if let Ok(mut guard) = ANDROID_APP.write() {
            *guard = Some(AndroidAppHandles {
                vm_ptr: ptr1,
                activity_ptr: ptr1,
            });
        }
        assert_eq!(ANDROID_APP.read().unwrap().unwrap().activity_ptr, ptr1);

        if let Ok(mut guard) = ANDROID_APP.write() {
            *guard = Some(AndroidAppHandles {
                vm_ptr: ptr2,
                activity_ptr: ptr2,
            });
        }
        assert_eq!(ANDROID_APP.read().unwrap().unwrap().activity_ptr, ptr2);

        unregister_android_app();
        assert!(ANDROID_APP.read().unwrap().is_none());
    }
}
