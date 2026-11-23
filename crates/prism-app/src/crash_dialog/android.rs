//! Android crash dialog `AlertDialog` via JNI + logcat + clipboard copying.
//!
//! The render thread (== winit event-loop / main thread on Android) holds
//! pointers to the JVM (`JavaVM*`) and the `Activity` jobject (registered by
//! [`super::register_android_app`]). We attach the current thread to the JVM
//! and, on the Activity's UI thread, build and show an `AlertDialog` with a
//! single OK button The dialog is modal from user's perspective: we
//! poll until the dialog is dismissed (or activity finishes) and block the
//! calling thread with a condvar.
//!
//! The complete error text is also written to logcat (tag `Prisma`) and copied
//! to system clipboard via `ClipboardManager`.

use std::sync::{Arc, Condvar, Mutex};

use jni::jni_sig;
use jni::jni_str;
use jni::objects::{JObject, JString, JValue};
use jni::sys::{jint, JNI_OK};
use jni::JavaVM;

use super::{CrashChoice, ANDROID_APP};

pub fn show(title: &str, message: &str) -> CrashChoice {
    log::error!("Crash dialog: {title}");

    let handles = match ANDROID_APP.read() {
        Ok(guard) => match *guard {
            Some(h) => h,
            None => {
                log::warn!("AndroidApp not registered; cannot show dialog, exiting");
                return CrashChoice::Exit;
            }
        },
        Err(_) => {
            log::warn!("ANDROID_APP lock poisoned; cannot show dialog, exiting");
            return CrashChoice::Exit;
        }
    };

    let vm = unsafe { JavaVM::from_raw(handles.vm_ptr as *mut _) };

    let done: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(false), Condvar::new()));
    let done_clone = Arc::clone(&done);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.attach_current_thread(|env| {
            let activity = unsafe { JObject::from_raw(env, handles.activity_ptr as *mut _) };
            let j_title = env.new_string(title)?;
            let j_message = env.new_string(message)?;
            show_alert_dialog(env, &activity, &j_title, &j_message, &done_clone)
        })
    }));
    if let Err(p) = result {
        log::warn!("AlertDialog JNI panic: {p:?}");
        return CrashChoice::Exit;
    }
    match result.unwrap() {
        Ok(()) => {}
        Err(e) => {
            log::warn!("AlertDialog JNI failed: {e}");
            return CrashChoice::Exit;
        }
    }

    let (lock, cvar) = &*done;
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    while !*guard {
        guard = cvar.wait(guard).unwrap_or_else(|p| p.into_inner());
    }

    CrashChoice::CopyAndExit
}

/// Build and show an `AlertDialog` with a single OK button. When OK is tapped
/// we call back into Rust through a registered native method that signals
/// `done`.
fn show_alert_dialog(
    env: &mut jni::Env<'_>,
    activity: &JObject,
    title: &JString,
    message: &JString,
    done: &Arc<(Mutex<bool>, Condvar)>,
) -> jni::errors::Result<()> {
    let builder_class = env.find_class(jni_str!("android/app/AlertDialog$Builder"))?;
    let builder = env.new_object(
        builder_class,
        jni_sig!("(Landroid/content/Context;)V"),
        &[JValue::Object(activity)],
    )?;

    env.call_method(
        &builder,
        jni_str!("setTitle"),
        jni_sig!("(Ljava/lang/CharSequence;)Landroid/app/AlertDialog$Builder;"),
        &[JValue::Object(title)],
    )?;
    env.call_method(
        &builder,
        jni_str!("setMessage"),
        jni_sig!("(Ljava/lang/CharSequence;)Landroid/app/AlertDialog$Builder;"),
        &[JValue::Object(message)],
    )?;
    env.call_method(
        &builder,
        jni_str!("setCancelable"),
        jni_sig!("(Z)Landroid/app/AlertDialog$Builder;"),
        &[JValue::Bool(false)],
    )?;

    let null_listener = JObject::null();
    let ok_label = env.new_string("OK")?;
    env.call_method(
        &builder,
        jni_str!("setPositiveButton"),
        jni_sig!("(Ljava/lang/CharSequence;Landroid/content/DialogInterface$OnClickListener;)Landroid/app/AlertDialog$Builder;"),
        &[JValue::Object(&ok_label), JValue::Object(&null_listener)],
    )?;

    let dialog = env.call_method(
        &builder,
        jni_str!("create"),
        jni_sig!("()Landroid/app/AlertDialog;"),
        &[],
    )?;
    let dialog_obj = dialog.l()?;

    env.call_method(&dialog_obj, jni_str!("show"), jni_sig!("()V"), &[])?;

    let global_dialog = env.new_global_ref(dialog_obj)?;

    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let is_showing = env
            .call_method(&global_dialog, jni_str!("isShowing"), jni_sig!("()Z"), &[])
            .and_then(|v| v.z())
            .unwrap_or(false);
        if !is_showing {
            break;
        }
        let is_finishing = env
            .call_method(activity, jni_str!("isFinishing"), jni_sig!("()Z"), &[])
            .and_then(|v| v.z())
            .unwrap_or(true);
        if is_finishing {
            break;
        }
    }

    let (lock, cvar) = &**done;
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    *guard = true;
    cvar.notify_one();

    let _: jint = JNI_OK;
    Ok(())
}

/// Copy `text` to Android system clipboard via `ClipboardManager` and `ClipData`.
pub fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let handles = match ANDROID_APP.read() {
        Ok(guard) => match *guard {
            Some(h) => h,
            None => return Ok(()),
        },
        Err(_) => return Ok(()),
    };
    let vm = unsafe { JavaVM::from_raw(handles.vm_ptr as *mut _) };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.attach_current_thread(|env| {
            let activity = unsafe { JObject::from_raw(env, handles.activity_ptr as *mut _) };
            copy_to_clipboard_jni(env, &activity, text)
        })
    }));
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            log::warn!("Android clipboard JNI failed: {e}");
            Ok(())
        }
        Err(p) => {
            log::warn!("Android clipboard JNI panic: {p:?}");
            Ok(())
        }
    }
}

fn copy_to_clipboard_jni(
    env: &mut jni::Env<'_>,
    activity: &JObject,
    text: &str,
) -> jni::errors::Result<()> {
    let service_name = env.new_string("clipboard")?;
    let clipboard_manager = env
        .call_method(
            activity,
            jni_str!("getSystemService"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
            &[JValue::Object(&service_name)],
        )?
        .l()?;

    if clipboard_manager.as_raw().is_null() {
        return Ok(());
    }

    let clip_data_class = env.find_class(jni_str!("android/content/ClipData"))?;
    let label = env.new_string("Prisma Crash Log")?;
    let text_j = env.new_string(text)?;
    let clip = env
        .call_static_method(
            clip_data_class,
            jni_str!("newPlainText"),
            jni_sig!(
                "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"
            ),
            &[JValue::Object(&label), JValue::Object(&text_j)],
        )?
        .l()?;

    env.call_method(
        &clipboard_manager,
        jni_str!("setPrimaryClip"),
        jni_sig!("(Landroid/content/ClipData;)V"),
        &[JValue::Object(&clip)],
    )?;
    Ok(())
}
