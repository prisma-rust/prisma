//! Tauri outer shell for Prisma Android launcher.
//!
//! This only keeps the APK "start game" page:
//!
//! - **Android**: launch `com.prisma.MainActivity` (GameActivity) via Kotlin plugin;
//! - **Desktop**: spawn local game binary, so launch page and parameter passing can be validated without packaging.

#[cfg(not(mobile))]
mod game_launcher;

use tauri::Wry;

#[cfg(mobile)]
use tauri::plugin::PluginHandle;
#[cfg(mobile)]
use tauri::Manager;

#[cfg(mobile)]
const NATIVE_PLUGIN_IDENTIFIER: &str = "com.example.tauriandroidapp";

/// Wrap calls to the correct Kotlin plugin.
#[cfg(mobile)]
struct NativeApi(PluginHandle<Wry>);

#[cfg(mobile)]
impl NativeApi {
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        payload: impl serde::Serialize,
    ) -> Result<T, String> {
        self.0
            .run_mobile_plugin(method, payload)
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
#[cfg(mobile)]
fn toggle_immersive(state: tauri::State<NativeApi>) -> Result<bool, String> {
    let result: serde_json::Value = state.call("toggle_immersive", ())?;
    result
        .get("immersive")
        .and_then(|v| v.as_bool())
        .ok_or("Kotlin returned malformed format".to_string())
}

#[tauri::command]
#[cfg(mobile)]
fn set_status_bar_color(state: tauri::State<NativeApi>, color: String) -> Result<(), String> {
    state.call::<serde_json::Value>(
        "set_status_bar_color",
        serde_json::json!({ "color": color }),
    )?;
    Ok(())
}

#[tauri::command]
#[cfg(mobile)]
fn vibrate(state: tauri::State<NativeApi>, duration: u64) -> Result<(), String> {
    state.call::<serde_json::Value>("vibrate", serde_json::json!({ "duration": duration }))?;
    Ok(())
}

#[tauri::command]
#[cfg(mobile)]
fn get_device_info(state: tauri::State<NativeApi>) -> Result<serde_json::Value, String> {
    state.call("get_device_info", ())
}

#[cfg(not(mobile))]
#[tauri::command]
fn toggle_immersive() -> Result<bool, String> {
    Err("Fullscreen mode only available on Android".into())
}

#[cfg(not(mobile))]
#[tauri::command]
fn set_status_bar_color(_color: String) -> Result<(), String> {
    Err("Status bar control only available on Android".into())
}

#[cfg(not(mobile))]
#[tauri::command]
fn vibrate(_duration: u64) -> Result<(), String> {
    Err("Vibration only available on Android".into())
}

#[cfg(not(mobile))]
#[tauri::command]
fn get_device_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "platform": "desktop",
                "message": "Device info only available on Android"
    }))
}

/// Launch game: mobile starts GameActivity via Kotlin plugin; desktop spawns
/// `template` binary. `config` is optional LaunchConfig JSON (scene / log level etc.),
/// passed via `PRISMA_LAUNCH_CONFIG` env (desktop) or files-dir file (Android).
#[cfg(mobile)]
#[tauri::command]
fn launch_game(state: tauri::State<NativeApi>, config: Option<String>) -> Result<(), String> {
    state
        .call::<serde_json::Value>("launch_game", serde_json::json!({ "config": config }))
        .map(|_| ())
}

/// Desktop: binary located by [`game_launcher::resolve`] (not via `PATH`),
/// rejects immediately if a game is already running, to avoid two instances.
#[cfg(not(mobile))]
#[tauri::command]
fn launch_game(
    state: tauri::State<'_, game_launcher::RunningGame>,
    config: Option<String>,
) -> Result<(), String> {
    state.launch(config)
}

/// Initialize custom native plugin (register Kotlin plugin on mobile).
pub fn init_native_plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri::plugin::Builder::<Wry>::new("native-api")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin(NATIVE_PLUGIN_IDENTIFIER, "NativePlugin")?;
                _app.manage(NativeApi(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(init_native_plugin());

    #[cfg(not(mobile))]
    let builder = builder.manage(game_launcher::RunningGame::default());

    builder
        .invoke_handler(tauri::generate_handler![
            launch_game,
            toggle_immersive,
            set_status_bar_color,
            vibrate,
            get_device_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Prisma launcher");
}
