//! One-shot sound effect request queue (#987).
//!
//! Project code (`GameWorld::play_sound`) only enqueues requests here, host takes them each frame and actually
//! plays: host backend only borrows `World`, cannot reach audio device; editor Play uses ABI callbacks,
//! not this path. This module does not depend on `prism-audio`, so disabling `audio` feature still compiles.

/// One-shot sound request submitted by project code (#987).
///
/// `GameWorld::play_sound` only **records** request: mixer (`App`) takes it at end of tick and
/// actually plays, because `GameWorld` host backend only borrows `World`, cannot reach audio device.
/// Editor Play goes through ABI callbacks instead (module and host are two engine instances).
#[derive(Debug, Clone, PartialEq)]
pub enum SoundCommand {
    /// Play `path`; `id` assigned by [`SoundCommands`], used for `stop_sound`.
    Play {
        id: u32,
        path: String,
        volume: f32,
        repeat: bool,
    },
    /// Stop a given id (unknown id ignored by host).
    Stop { id: u32 },
}

/// Queue resource of [`SoundCommand`].
#[derive(Debug, Default)]
pub struct SoundCommands {
    commands: Vec<SoundCommand>,
    next_id: u32,
}

impl SoundCommands {
    /// Enqueue one play, return assigned sound id (starts from 1, `0` reserved as "invalid").
    pub fn play(&mut self, path: impl Into<String>, volume: f32, repeat: bool) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.commands.push(SoundCommand::Play {
            id,
            path: path.into(),
            volume: if volume.is_finite() {
                volume.clamp(0.0, 1.0)
            } else {
                1.0
            },
            repeat,
        });
        id
    }

    /// Enqueue stop for a given id.
    pub fn stop(&mut self, id: u32) {
        if id != 0 {
            self.commands.push(SoundCommand::Stop { id });
        }
    }

    /// Take this frame queued requests (host calls once per frame).
    pub fn take(&mut self) -> Vec<SoundCommand> {
        std::mem::take(&mut self.commands)
    }

    /// How many requests not yet taken (for tests / diagnostics).
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}
