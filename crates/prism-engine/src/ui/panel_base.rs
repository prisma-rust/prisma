use crate::util::timer::{TimerClient, TimerId, TimerParams, TimerSet};

/// UI panel base class — holds TimerSet + child panel tree + visibility.
pub struct PanelBase {
    /// Timer set (auto-caches ids, batch destroy on Drop).
    pub timers: TimerSet,
    /// Child panels.
    pub children: Vec<Box<dyn super::Panel>>,
    /// Whether visible / active.
    pub visible: bool,
}

impl PanelBase {
    pub fn new(client: TimerClient) -> Self {
        Self {
            timers: TimerSet::new(client),
            children: Vec::new(),
            visible: true,
        }
    }

    // ── Timer forwarding ─────────────────────────────────────────

    /// Create timer (auto-caches id, auto-cleaned when panel drops).
    pub fn create_timer(&mut self, params: TimerParams) -> TimerId {
        self.timers.create_timer(params)
    }

    pub fn destroy_timer(&mut self, id: TimerId) {
        self.timers.destroy(id);
    }

    pub fn pause_timer(&self, id: TimerId) {
        self.timers.pause(id);
    }

    pub fn resume_timer(&self, id: TimerId) {
        self.timers.resume(id);
    }

    /// Clear all timers of this panel (called when disabling panel).
    pub fn clear_timers(&mut self) {
        self.timers.clear();
    }
}
