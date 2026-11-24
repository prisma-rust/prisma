//! Scene audio: connect [`AudioSource`] in ECS to mixer.
//!
//! Layered consistent with `MeshRenderer -> MeshRef`: `AudioSource` only writes **authoring state**
//! (fragment path / volume / loop / distance attenuation), this module responsible for
//!
//! 1. Find files by path under `assets/`, hand to background decode thread (`audio_decode_runner`);
//! 2. Decode results cached by path, same fragment decoded only once;
//! 3. One "voice" per entity: `play_on_awake` + fragment plays when ready, then every frame sync volume,
//!    loop repeat, stop when entity disappears.
//!
//! Mixer goes through [`AudioSink`] abstraction, so this state machine can be unit-tested without sound card environment
//! (see `tests` at end of file).
//!
//! # Status and boundaries
//!
//! - **Distance attenuation + equal-power panning**: `spatial_blend` simultaneously scales attenuation curve and pan strength.
//!   Every frame send "source offset relative to listener" (converted to listener space) to mixer
//!   (`AudioSink::set_spatial`), Firewheel's `SpatialBasicNode` does equal-power panning
//!   and smooth transition, so moving sources pan left/right in real time, no need to re-play fragment for pan change.
//!   Distance attenuation still computed here (`AudioSource::volume_at`), node only handles pan.
//! - Only read **loose files**: audio under `assets/**`; audio inside `.pak` not yet connected.
//! - Fragment sample rate / channels provided by decoder; mixer responsible for resampling.
//! - Do not listen for file changes: need to restart or reload scene after changing audio (asset hot reload only handles `.pak`).

use std::collections::HashMap;

use flume::{Receiver, Sender};
use prism_audio::AudioData;
use prism_engine::ecs::{Entity, World};
use prism_engine::scene::components::{AudioBus, AudioListener, AudioSource, Camera, LocalToWorld};

use crate::audio_decode_runner::{DecodeRequest, DecodeResult};

/// Minimal mixer interface.
///
/// Implementer maintains mapping from `voice` (opaque id) to real playback handle; `0` means "play failed".
pub trait AudioSink {
    /// Start playing, return voice id (`0` = failed).
    fn play(&mut self, clip: &AudioData) -> u64;
    fn stop(&mut self, voice: u64);
    fn set_volume(&mut self, voice: u64, volume: f32);
    /// Real-time spatialization: `offset` is source offset relative to listener, converted to **listener space**
    /// (`+x` right, `+y` up, `+z` back), `panning_strength` is pan strength `0..=1`
    /// (`0` keeps center).
    ///
    /// Empty implementation provided to not break existing tests / custom sink; real mixer should override it.
    fn set_spatial(&mut self, _voice: u64, _offset: [f32; 3], _panning_threshold: f32) {}
    fn is_playing(&self, voice: u64) -> bool;
}

/// One entity's voice.
#[derive(Debug, Clone)]
struct Voice {
    /// Mixer voice id.
    id: u64,
    /// Volume already sent to mixer (avoid re-queueing event every frame).
    volume: f32,
    /// Listener-space offset already sent to mixer (#508, moving source updated every frame).
    offset: [f32; 3],
    /// Pan strength already sent to mixer (#1034, from `spatial_blend`).
    panning_threshold: f32,
    /// Whether audio thread really started playing (still `false` right after `play()` returns).
    started: bool,
    /// Fragment path corresponding to voice (replay when path changes).
    path: String,
}

/// Scene audio driver. Called once per frame by host (`App`).
pub struct SceneAudio {
    /// Decoded fragments (path -> samples).
    clips: HashMap<String, AudioData>,
    /// Paths with decode failure or being decoded (avoid re-queueing same frame).
    pending: HashMap<String, u64>,
    /// Request id -> path, used to map async results back to fragments.
    requests: HashMap<u64, String>,
    next_request: u64,
    voices: HashMap<Entity, Voice>,
    /// Gain per bus (index = [`AudioBus::index`], default all 1).
    bus_gains: [f32; 5],
}

impl Default for SceneAudio {
    fn default() -> Self {
        Self {
            clips: HashMap::new(),
            pending: HashMap::new(),
            requests: HashMap::new(),
            next_request: 0,
            voices: HashMap::new(),
            bus_gains: [1.0; 5],
        }
    }
}

impl SceneAudio {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set gain for one bus (`0..=1`, non-finite ignored).
    ///
    /// `Master` multiplies all other buses; playing voices update volume next frame with new gain.
    pub fn set_bus_volume(&mut self, bus: AudioBus, volume: f32) {
        if volume.is_finite() {
            self.bus_gains[bus.index()] = volume.clamp(0.0, 1.0);
        }
    }

    /// Read one bus's own gain (excluding `Master`).
    pub fn bus_volume(&self, bus: AudioBus) -> f32 {
        self.bus_gains[bus.index()]
    }

    /// Total gain actually effective for a voice: `Master` × its own bus.
    pub fn bus_gain(&self, bus: AudioBus) -> f32 {
        let master = self.bus_gains[AudioBus::Master.index()];
        if bus == AudioBus::Master {
            master
        } else {
            master * self.bus_gains[bus.index()]
        }
    }

    /// Number of cached fragments (diagnostic / test).
    pub fn cached_clips(&self) -> usize {
        self.clips.len()
    }

    /// Number of voices currently playing (diagnostic / test).
    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// Directly inject a decoded fragment (test / preload sound in code).
    pub fn insert_clip(&mut self, path: &str, clip: AudioData) {
        self.add_clip(path, clip);
    }

    /// Accept decode thread result: on success cache it.
    pub fn receive(&mut self, result: DecodeResult) {
        match result {
            DecodeResult::Decoded { request_id, data } => {
                let Some(path) = self.requests.remove(&request_id) else {
                    return;
                };
                self.pending.remove(&path);
                self.add_clip(&path, data);
            }
            DecodeResult::Error { request_id, .. } => {
                if let Some(path) = self.requests.remove(&request_id) {
                    self.pending.remove(&path);
                }
            }
        }
    }

    /// Stop all voices (called on scene unload / shutdown).
    pub fn stop_all(&mut self, sink: &mut dyn AudioSink) {
        for (_, voice) in self.voices.drain() {
            sink.stop(voice.id);
        }
    }

    /// Per-frame sync: play new voices, update volume, loop repeat, recycle disappeared entities.
    ///
    /// `requests` is decode thread input channel (`None` = no decode thread, only play cached fragments).
    pub fn update(
        &mut self,
        world: &World,
        sink: &mut dyn AudioSink,
        requests: Option<&Sender<DecodeRequest>>,
    ) {
        let listener = listener(world);
        let mut live: Vec<Entity> = Vec::new();

        let sources: Vec<(Entity, AudioSource)> = world
            .query::<AudioSource>()
            .map(|(entity, source)| (entity, source.clone()))
            .collect();

        for (entity, source) in sources {
            live.push(entity);
            if !source.has_sound() || !source.play_on_awake {
                if let Some(voice) = self.voices.remove(&entity) {
                    sink.stop(voice.id);
                }
                continue;
            }

            let position = world_position(world, entity);
            let volume = match (position, listener.position) {
                (Some(position), Some(listener_position)) => {
                    source.volume_at(distance(position, listener_position)) * listener.gain
                }
                _ => source.volume_at(0.0) * listener.gain,
            }
            .clamp(0.0, 1.0)
                * self.bus_gain(source.bus);
            let volume = volume.clamp(0.0, 1.0);
            let threshold = panning_threshold(source.spatial_blend);
            let (offset, panning_threshold) = match (position, listener.position) {
                (Some(position), Some(listener_position)) => (
                    listener_offset(position, listener_position, listener.basis),
                    threshold,
                ),
                _ => ([0.0; 3], 0.0),
            };

            if !self.clips.contains_key(&source.sound_path) {
                self.request(&source.sound_path, requests);
                continue;
            }

            if let Some(voice) = self.voices.get(&entity) {
                if voice.path != source.sound_path {
                    let voice = self.voices.remove(&entity).expect("checked above");
                    sink.stop(voice.id);
                }
            }

            match self.voices.get_mut(&entity) {
                None => {
                    let Some(clip) = self.clips.get(&source.sound_path) else {
                        continue;
                    };
                    let path = source.sound_path.clone();
                    let id = sink.play(clip);
                    if id == 0 {
                        continue;
                    }
                    sink.set_volume(id, volume);
                    sink.set_spatial(id, offset, panning_threshold);
                    self.voices.insert(
                        entity,
                        Voice {
                            id,
                            volume,
                            offset,
                            panning_threshold,
                            started: false,
                            path,
                        },
                    );
                }
                Some(voice) => {
                    if sink.is_playing(voice.id) {
                        voice.started = true;
                    } else if voice.started {
                        if !source.looped {
                            let voice = self.voices.remove(&entity).expect("checked above");
                            sink.stop(voice.id);
                            continue;
                        }
                        let Some(clip) = self.clips.get(&source.sound_path) else {
                            continue;
                        };
                        let path = source.sound_path.clone();
                        let id = sink.play(clip);
                        if id == 0 {
                            continue;
                        }
                        sink.set_volume(id, volume);
                        sink.set_spatial(id, offset, panning_threshold);
                        self.voices.insert(
                            entity,
                            Voice {
                                id,
                                volume,
                                offset,
                                panning_threshold,
                                started: false,
                                path,
                            },
                        );
                        continue;
                    }
                    if (voice.volume - volume).abs() > 0.001 {
                        voice.volume = volume;
                        sink.set_volume(voice.id, volume);
                    }
                    let offset_changed = voice
                        .offset
                        .iter()
                        .zip(offset.iter())
                        .any(|(old, new)| (old - new).abs() > 0.001);
                    if offset_changed || (voice.panning_threshold - panning_threshold).abs() > 0.001
                    {
                        voice.offset = offset;
                        voice.panning_threshold = panning_threshold;
                        sink.set_spatial(voice.id, offset, panning_threshold);
                    }
                }
            }
        }

        let stale: Vec<Entity> = self
            .voices
            .keys()
            .filter(|entity| !live.contains(entity))
            .copied()
            .collect();
        for entity in stale {
            if let Some(voice) = self.voices.remove(&entity) {
                sink.stop(voice.id);
            }
        }
    }

    fn add_clip(&mut self, path: &str, clip: AudioData) {
        self.clips.insert(path.to_string(), clip);
    }

    fn request(&mut self, path: &str, requests: Option<&Sender<DecodeRequest>>) {
        if self.pending.contains_key(path) {
            return;
        }
        let Some(requests) = requests else {
            return;
        };
        if prism_engine::asset_paths::strict_pak_only() {
            return;
        }
        let Some(file) = prism_engine::asset_paths::asset_path(path) else {
            log::warn!("audio clip not found in assets/: {path}");
            return;
        };
        self.next_request += 1;
        let request_id = self.next_request;
        self.pending.insert(path.to_string(), request_id);
        self.requests.insert(request_id, path.to_string());
        if let Err(error) = requests.send(DecodeRequest::DecodeFile {
            path: file.to_string_lossy().into_owned(),
            request_id,
        }) {
            log::warn!("audio decode thread is gone: {error}");
            self.pending.remove(path);
            self.requests.remove(&request_id);
        }
    }
}

/// Listener: position, gain and **basis vectors** (for panning).
struct Listener {
    position: Option<[f32; 3]>,
    gain: f32,
    /// Right, up, back three world-space unit vectors.
    basis: [[f32; 3]; 3],
}

/// Listener position / gain / basis vectors: prefer `AudioListener` entity, otherwise fall back to first camera,
/// otherwise origin + world axes.
fn listener(world: &World) -> Listener {
    let gain = |entity: Entity, fallback: f32| {
        world
            .get::<AudioListener>(entity)
            .map(|listener| listener.gain.clamp(0.0, 1.0))
            .unwrap_or(fallback)
    };
    if let Some((entity, _)) = world.query::<AudioListener>().next() {
        return Listener {
            position: world_position(world, entity),
            gain: gain(entity, 1.0),
            basis: listener_basis(world, entity),
        };
    }
    if let Some((entity, _)) = world.query::<Camera>().next() {
        return Listener {
            position: world_position(world, entity),
            gain: 1.0,
            basis: listener_basis(world, entity),
        };
    }
    Listener {
        position: Some([0.0; 3]),
        gain: 1.0,
        basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    }
}

/// Listener right / up / back vectors (world space, normalized; degenerate axis falls back to world axes).
fn listener_basis(world: &World, entity: Entity) -> [[f32; 3]; 3] {
    let mut axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let Some(transform) = world.get::<LocalToWorld>(entity) else {
        return axes;
    };
    let columns = [transform.0.x_axis, transform.0.y_axis, transform.0.z_axis];
    for (axis, column) in axes.iter_mut().zip(columns) {
        let column = column.truncate();
        let length = column.length();
        if length.is_finite() && length > 1e-5 {
            *axis = (column / length).to_array();
        }
    }
    axes
}

/// Source offset relative to listener, converted to listener space (`+x` right, `+y` up, `+z` back).
///
/// Firewheel's `SpatialBasicNode` does equal-power panning using horizontal angle of `+x` direction, and puts
/// offset length as distance, so here only need to project world-space offset onto listener's basis vectors.
fn listener_offset(source: [f32; 3], listener: [f32; 3], basis: [[f32; 3]; 3]) -> [f32; 3] {
    let offset = [
        source[0] - listener[0],
        source[1] - listener[1],
        source[2] - listener[2],
    ];
    if !offset.iter().all(|value| value.is_finite()) {
        return [0.0; 3];
    }
    [
        dot(offset, basis[0]),
        dot(offset, basis[1]),
        dot(offset, basis[2]),
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `spatial_blend` -> pan strength; non-finite treated as 0, `0` means keep center (2D source).
fn panning_threshold(spatial_blend: f32) -> f32 {
    if spatial_blend.is_finite() {
        spatial_blend.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn world_position(world: &World, entity: Entity) -> Option<[f32; 3]> {
    world
        .get::<LocalToWorld>(entity)
        .map(|world| world.0.w_axis.truncate().to_array())
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Map [`AudioSink`]'s `u64` voice to real [`prism_audio::PlaybackHandle`].
#[derive(Debug, Default)]
pub struct VoiceHandles {
    handles: HashMap<u64, prism_audio::PlaybackHandle>,
    next: u64,
}

impl AudioSink for (&mut prism_audio::AudioEngine, &mut VoiceHandles) {
    fn play(&mut self, clip: &AudioData) -> u64 {
        let handle = self.0.play(clip);
        if !handle.is_valid() {
            return 0;
        }
        self.1.next += 1;
        let id = self.1.next;
        self.1.handles.insert(id, handle);
        id
    }

    fn stop(&mut self, voice: u64) {
        if let Some(handle) = self.1.handles.remove(&voice) {
            self.0.stop(&handle);
        }
    }

    fn set_volume(&mut self, voice: u64, volume: f32) {
        if let Some(handle) = self.1.handles.get(&voice) {
            self.0.set_volume(handle, volume);
        }
    }

    fn set_spatial(&mut self, voice: u64, offset: [f32; 3], panning_threshold: f32) {
        if let Some(handle) = self.1.handles.get(&voice) {
            self.0.set_spatial(handle, offset, panning_threshold);
        }
    }

    fn is_playing(&self, voice: u64) -> bool {
        self.1
            .handles
            .get(&voice)
            .map(|handle| self.0.is_playing(handle))
            .unwrap_or(false)
    }
}

/// Decode result distribution: host hands `DecodeResult` to [`SceneAudio::receive`] every frame.
pub fn drain_decoded(scene_audio: &mut SceneAudio, results: &Receiver<DecodeResult>) -> usize {
    let mut drained = 0;
    while let Ok(result) = results.try_recv() {
        scene_audio.receive(result);
        drained += 1;
    }
    drained
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;
    use std::time::Duration;

    /// Mock recording mixer: records calls, `is_playing` can be manually changed.
    #[derive(Default)]
    struct FakeSink {
        next: u64,
        ends: HashMap<u64, &'static str>,
        playing: HashMap<u64, bool>,
        volumes: HashMap<u64, f32>,
        spatial: HashMap<u64, ([f32; 3], f32)>,
        stopped: Vec<u64>,
        played: usize,
    }

    impl FakeSink {
        fn finish(&mut self, voice: u64) {
            self.playing.insert(voice, false);
        }
        fn volumes(&self) -> Vec<f32> {
            let mut values: Vec<(u64, f32)> = self.volumes.iter().map(|(k, v)| (*k, *v)).collect();
            values.sort_by_key(|a| a.0);
            values.into_iter().map(|(_, volume)| volume).collect()
        }
        fn spatials(&self) -> Vec<([f32; 3], f32)> {
            let mut values: Vec<(u64, ([f32; 3], f32))> =
                self.spatial.iter().map(|(k, v)| (*k, *v)).collect();
            values.sort_by_key(|a| a.0);
            values.into_iter().map(|(_, spatial)| spatial).collect()
        }
    }

    impl AudioSink for FakeSink {
        fn play(&mut self, clip: &AudioData) -> u64 {
            assert!(
                !clip.samples.is_empty(),
                "empty fragment should not start playing"
            );
            self.next += 1;
            let id = self.next;
            self.played += 1;
            self.playing.insert(id, true);
            self.ends.insert(id, "clip");
            id
        }
        fn stop(&mut self, voice: u64) {
            self.playing.insert(voice, false);
            self.stopped.push(voice);
        }
        fn set_volume(&mut self, voice: u64, volume: f32) {
            self.volumes.insert(voice, volume);
        }
        fn set_spatial(&mut self, voice: u64, offset: [f32; 3], panning_threshold: f32) {
            self.spatial.insert(voice, (offset, panning_threshold));
        }
        fn is_playing(&self, voice: u64) -> bool {
            self.playing.get(&voice).copied().unwrap_or(false)
        }
    }

    fn clip() -> AudioData {
        AudioData {
            samples: vec![0.0; 16],
            sample_rate: 44_100,
            channels: 1,
            duration: Duration::from_secs(1),
        }
    }

    fn place(world: &mut World, position: [f32; 3]) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            LocalToWorld(Mat4::from_translation(glam::Vec3::from_array(position))),
        );
        entity
    }

    fn source(world: &mut World, entity: Entity, source: AudioSource) {
        world.insert(entity, source);
    }

    fn plain(path: &str) -> AudioSource {
        AudioSource {
            sound_path: path.into(),
            ..AudioSource::default()
        }
    }

    #[test]
    fn bus_gains_multiply_the_source_volume_and_master_scales_every_bus() {
        let mut world = World::new();
        place(&mut world, [0.0; 3]);
        let speaker = place(&mut world, [0.0; 3]);
        source(
            &mut world,
            speaker,
            AudioSource {
                volume: 0.8,
                bus: AudioBus::Music,
                ..plain("audio/music.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/music.ogg", clip());
        let mut sink = FakeSink::default();

        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.8).abs() < 1e-6);

        scene.set_bus_volume(AudioBus::Music, 0.5);
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.4).abs() < 1e-6);

        scene.set_bus_volume(AudioBus::Master, 0.5);
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.2).abs() < 1e-6);

        scene.set_bus_volume(AudioBus::Sfx, 0.1);
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.2).abs() < 1e-6);

        scene.set_bus_volume(AudioBus::Music, f32::NAN);
        assert_eq!(scene.bus_volume(AudioBus::Music), 0.5);
        assert_eq!(scene.bus_gain(AudioBus::Master), 0.5);
    }

    #[test]
    fn a_clip_plays_once_and_follows_the_distance_volume() {
        let mut world = World::new();
        place(&mut world, [0.0; 3]);
        let speaker = place(&mut world, [0.0, 0.0, 5.0]);
        source(
            &mut world,
            speaker,
            AudioSource {
                spatial_blend: 1.0,
                min_distance: 0.0,
                max_distance: 10.0,
                volume: 1.0,
                ..plain("audio/beep.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/beep.ogg", clip());
        let mut sink = FakeSink::default();

        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 1, "fragment plays once");
        assert_eq!(scene.active_voices(), 1);
        assert!(
            (sink.volumes()[0] - 0.5).abs() < 1e-6,
            "distance 5/10 -> half volume"
        );

        world.insert(
            speaker,
            LocalToWorld(Mat4::from_translation(glam::Vec3::new(0.0, 0.0, 2.5))),
        );
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 1);
        assert!((sink.volumes()[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn offsets_are_expressed_in_listener_space() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(
            listener_offset([5.0, 0.0, 0.0], [0.0; 3], identity),
            [5.0, 0.0, 0.0]
        );
        let facing_x = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        assert_eq!(
            listener_offset([5.0, 0.0, 0.0], [0.0; 3], facing_x),
            [0.0, 0.0, 5.0]
        );
        assert_eq!(
            listener_offset([f32::NAN, 0.0, 0.0], [0.0; 3], identity),
            [0.0; 3]
        );
        assert_eq!(panning_threshold(0.0), 0.0);
        assert_eq!(panning_threshold(f32::NAN), 0.0);
        assert_eq!(panning_threshold(2.0), 1.0);
        assert_eq!(panning_threshold(0.5), 0.5);
    }

    #[test]
    fn a_source_to_the_right_keeps_its_offset_on_x() {
        let mut world = World::new();
        let listener_entity = place(&mut world, [0.0; 3]);
        world.insert(listener_entity, Camera::default());
        let speaker = place(&mut world, [5.0, 0.0, 0.0]);
        source(
            &mut world,
            speaker,
            AudioSource {
                spatial_blend: 1.0,
                min_distance: 0.0,
                max_distance: 10.0,
                volume: 1.0,
                ..plain("audio/beep.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/beep.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);

        assert_eq!(sink.spatials(), vec![([5.0, 0.0, 0.0], 1.0)]);
    }

    #[test]
    fn a_moving_source_updates_the_offset_without_replaying_the_clip() {
        let mut world = World::new();
        place(&mut world, [0.0; 3]);
        let speaker = place(&mut world, [5.0, 0.0, 0.0]);
        source(
            &mut world,
            speaker,
            AudioSource {
                spatial_blend: 1.0,
                min_distance: 0.0,
                max_distance: 10.0,
                volume: 1.0,
                ..plain("audio/beep.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/beep.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 1);
        assert_eq!(sink.spatials(), vec![([5.0, 0.0, 0.0], 1.0)]);

        world.insert(
            speaker,
            LocalToWorld(Mat4::from_translation(glam::Vec3::new(-5.0, 0.0, 0.0))),
        );
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 1, "panning must not restart the voice");
        assert_eq!(sink.spatials(), vec![([-5.0, 0.0, 0.0], 1.0)]);
    }

    #[test]
    fn a_2d_source_ignores_the_distance() {
        let mut world = World::new();
        let speaker = place(&mut world, [0.0, 0.0, 500.0]);
        source(
            &mut world,
            speaker,
            AudioSource {
                volume: 0.4,
                ..plain("audio/ui.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/ui.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.4).abs() < 1e-6);
        assert_eq!(
            sink.spatials(),
            vec![([0.0, 0.0, 500.0], 0.0)],
            "a 2D source keeps the panning threshold at zero"
        );
    }

    #[test]
    fn the_listener_gain_scales_every_source() {
        let mut world = World::new();
        let listener = place(&mut world, [0.0; 3]);
        world.insert(listener, AudioListener { gain: 0.5 });
        let speaker = place(&mut world, [0.0; 3]);
        source(&mut world, speaker, plain("audio/beep.ogg"));

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/beep.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_looped_source_restarts_but_a_one_shot_is_released() {
        let mut world = World::new();
        let looped = place(&mut world, [0.0; 3]);
        source(
            &mut world,
            looped,
            AudioSource {
                looped: true,
                ..plain("audio/loop.ogg")
            },
        );
        let one_shot = place(&mut world, [0.0; 3]);
        source(&mut world, one_shot, plain("audio/hit.ogg"));

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/loop.ogg", clip());
        scene.insert_clip("audio/hit.ogg", clip());
        let mut sink = FakeSink::default();

        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 2);
        assert_eq!(scene.active_voices(), 2);

        scene.update(&world, &mut sink, None);
        for voice in 1..=2 {
            sink.finish(voice);
        }
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 3, "only looping one re-plays");
        assert_eq!(scene.active_voices(), 1);
        assert_eq!(sink.stopped.len(), 1, "one-shot voice recycled");
    }

    #[test]
    fn clearing_the_component_or_despawning_stops_the_voice() {
        let mut world = World::new();
        let a = place(&mut world, [0.0; 3]);
        source(&mut world, a, plain("audio/a.ogg"));
        let b = place(&mut world, [0.0; 3]);
        source(&mut world, b, plain("audio/a.ogg"));

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/a.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert_eq!(scene.active_voices(), 2);

        world.insert(
            a,
            AudioSource {
                sound_path: String::new(),
                ..AudioSource::default()
            },
        );
        world.despawn(b);
        scene.update(&world, &mut sink, None);
        assert_eq!(scene.active_voices(), 0);
        assert_eq!(sink.stopped.len(), 2);
    }

    #[test]
    fn a_missing_clip_does_not_block_the_voice_slot() {
        let mut world = World::new();
        let speaker = place(&mut world, [0.0; 3]);
        source(&mut world, speaker, plain("audio/not-cached.ogg"));

        let mut scene = SceneAudio::new();
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 0);
        assert_eq!(
            scene.active_voices(),
            0,
            "no fragment means no voice occupied"
        );
    }
}
