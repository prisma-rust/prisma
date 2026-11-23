//! One-shot sound (#987): connect `GameWorld::play_sound` queued requests to mixer.
//!
//! Layered and scene audio consistent: project code only **queues** (`prism_engine::sound::SoundCommands`),
//! this module really decodes + plays after `App` tick ends, so `GameWorld` host backend
//! does not need to touch audio device; plugin (editor Play) walks ABI callback, two paths independent.
//!
//! Cost is decode happens on request frame (sync file read). Short sounds (splash / hook hint) fine;
//! long music use `AudioSource` component, that path is async decode + path cached.

use std::collections::HashMap;

use prism_asset::runtime::ResourceManager;
use prism_audio::{AudioData, AudioEngine, PlaybackHandle};
use prism_engine::ecs::World;
use prism_engine::sound::{SoundCommand, SoundCommands};

/// One-shot sound currently playing.
pub struct OneShotSound {
    handle: PlaybackHandle,
    /// Retain clip for `repeat` (`AudioEngine` has no loop flag, replay after finish).
    clip: AudioData,
    volume: f32,
    repeat: bool,
}

impl OneShotSound {
    pub fn is_repeating(&self) -> bool {
        self.repeat
    }
}

/// Parse sound path: prefer loose files under `assets/`, then same-named resource in `.pak`.
///
/// Release package only has `.pak`, so pak branch is not optional: `build-scenes` will put
/// entire `assets/audio/**` directory into package (like `sprites/`, since runtime reads by path).
fn load_clip(relative: &str, pack: &mut ResourceManager) -> Option<AudioData> {
    if let Some(path) = prism_engine::asset_paths::asset_path(relative) {
        if path.is_file() {
            return match prism_audio::decoder::decode_file(&path) {
                Ok(clip) => Some(clip),
                Err(error) => {
                    log::warn!("play_sound: decode '{relative}' failed: {error}");
                    None
                }
            };
        }
    }
    let id = pack.id_by_path(relative)?;
    let bytes = pack.load_with_deps_raw(id).ok()?;
    match prism_audio::decoder::decode_auto(&bytes) {
        Ok(clip) => Some(clip),
        Err(error) => {
            log::warn!("play_sound: decode '{relative}' from the package failed: {error}");
            None
        }
    }
}

/// Take and execute this frame queued one-shot sound requests.
///
/// When `audio` is `None` (no sound card / `audio` feature not on) requests are dropped, warn once.
pub fn drain(
    world: &mut World,
    audio: Option<&mut AudioEngine>,
    sounds: &mut HashMap<u32, OneShotSound>,
    pack: &mut ResourceManager,
) {
    let Some(commands) = world.get_resource_mut::<SoundCommands>() else {
        return;
    };
    let commands: Vec<SoundCommand> = commands.take();
    if commands.is_empty() && sounds.is_empty() {
        return;
    }

    let Some(audio) = audio else {
        if !commands.is_empty() {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                log::warn!("play_sound: no audio device is available; sound requests are dropped");
            });
        }
        return;
    };

    for command in commands {
        match command {
            SoundCommand::Play {
                id,
                path,
                volume,
                repeat,
            } => {
                let Some(clip) = load_clip(&path, pack) else {
                    log::warn!("play_sound: '{path}' not found in assets/ or the package");
                    continue;
                };
                let handle = audio.play(&clip);
                if !handle.is_valid() {
                    log::warn!("play_sound: mixer refused '{path}'");
                    continue;
                }
                audio.set_volume(&handle, volume);
                if let Some(old) = sounds.remove(&id) {
                    audio.stop(&old.handle);
                }
                log::debug!(
                    "play_sound: '{path}' started as id {id} ({} frames, volume {volume:.2}, repeat {repeat})",
                    clip.samples.len() / clip.channels.max(1) as usize
                );
                sounds.insert(
                    id,
                    OneShotSound {
                        handle,
                        clip,
                        volume,
                        repeat,
                    },
                );
            }
            SoundCommand::Stop { id } => {
                if let Some(sound) = sounds.remove(&id) {
                    audio.stop(&sound.handle);
                }
            }
        }
    }

    for sound in sounds.values_mut() {
        if !sound.repeat || audio.is_playing(&sound.handle) {
            continue;
        }
        let handle = audio.play(&sound.clip);
        if handle.is_valid() {
            audio.set_volume(&handle, sound.volume);
            sound.handle = handle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_clip_is_skipped_without_panicking() {
        let mut world = World::new();
        let mut commands = SoundCommands::default();
        let id = commands.play("audio/does-not-exist.ogg", 1.0, false);
        world.insert_resource(commands);
        let mut sounds = HashMap::new();
        let mut pack = ResourceManager::new();
        drain(&mut world, None, &mut sounds, &mut pack);
        assert!(sounds.is_empty());
        assert!(world
            .get_resource::<SoundCommands>()
            .map(SoundCommands::is_empty)
            .unwrap_or(false));
        assert_ne!(id, 0);
    }
}
