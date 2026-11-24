//! Main-thread timer service — create / pause / resume / destroy.
//!
//! Storage uses SlotMap (Vec<Option<T>> + free list), u32 slot index as ID, no hash.
//!
//! # Usage
//! ```ignore
//! let mut timer = TimerService::new();
//! let h = timer.client.create_timer(TimerParams { .. });
//! timer.pause(h); timer.resume(h); timer.destroy(h);
//! timer.tick(); // Main loop every frame
//! ```

use std::{
    collections::HashMap,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

struct SlotMap<T> {
    pub(crate) slots: Vec<Option<T>>,
    pub(crate) free: Vec<u32>,
}

impl<T> SlotMap<T> {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    fn insert(&mut self, value: T) -> u32 {
        if let Some(idx) = self.free.pop() {
            self.slots[idx as usize] = Some(value);
            idx
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Some(value));
            idx
        }
    }

    fn get(&self, id: u32) -> Option<&T> {
        self.slots.get(id as usize)?.as_ref()
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.slots.get_mut(id as usize)?.as_mut()
    }

    fn remove(&mut self, id: u32) {
        if let Some(slot) = self.slots.get_mut(id as usize) {
            if slot.is_some() {
                *slot = None;
                self.free.push(id);
            }
        }
    }
}

type Callback = Box<dyn FnMut() + Send + 'static>;

struct TimerState {
    alive: bool,
    paused: bool,
    remaining: Duration,
    deadline: Option<Instant>,
    group: Option<u32>,
}

struct RegMsg {
    id: u32,
    group_id: Option<u32>,
    params: TimerParams,
}

/// Messages from [`TimerClient`] to the main-thread [`TimerService`].
enum ServiceMsg {
    Register(RegMsg),
    Destroy(TimerId),
    DestroyGroup(u32),
}

/// Timer creation parameters.
pub struct TimerParams {
    pub delay: Duration,
    pub interval: Option<Duration>,
    pub exec_immediately: bool,
    pub group_id: Option<u32>,
    pub callback: Callback,
}

/// u32 slot index, i.e. the timer id.
pub type TimerId = u32;

/// Timer client — Cloneable, UI layer holds it directly.
#[derive(Clone)]
pub struct TimerClient {
    reg_tx: Sender<ServiceMsg>,
    state: Arc<Mutex<SlotMap<TimerState>>>,
}

impl TimerClient {
    pub fn create_timer(&self, params: TimerParams) -> TimerId {
        let deadline = Instant::now() + params.delay;
        let id = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.insert(TimerState {
                alive: true,
                paused: false,
                remaining: params.delay,
                deadline: Some(deadline),
                group: params.group_id,
            })
        };
        let group_id = params.group_id;
        let _ = self.reg_tx.send(ServiceMsg::Register(RegMsg {
            id,
            group_id,
            params,
        }));
        id
    }

    pub fn destroy(&self, id: TimerId) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let _ = self.reg_tx.send(ServiceMsg::Destroy(id));
    }

    pub fn pause(&self, id: TimerId) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(st) = s.get_mut(id) {
            if st.alive && !st.paused {
                if let Some(deadline) = st.deadline.take() {
                    st.remaining = deadline.saturating_duration_since(Instant::now());
                }
                st.paused = true;
            }
        }
    }

    pub fn resume(&self, id: TimerId) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(st) = s.get_mut(id) {
            if st.alive && st.paused {
                st.paused = false;
                st.deadline = Some(Instant::now() + st.remaining);
            }
        }
    }

    pub fn destroy_group(&self, group: u32) {
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for i in 0..s.slots.len() {
                if let Some(ref st) = s.slots[i] {
                    if st.group == Some(group) {
                        s.slots[i] = None;
                        s.free.push(i as u32);
                    }
                }
            }
        }
        let _ = self.reg_tx.send(ServiceMsg::DestroyGroup(group));
    }
}

/// Main-thread timer service.
pub struct TimerService {
    pub client: TimerClient,
    reg_rx: Receiver<ServiceMsg>,
    /// Callback/params keyed by the same slot id as `client.state`.
    params: HashMap<TimerId, TimerParams>,
    groups: HashMap<u32, Vec<u32>>,
}

impl TimerService {
    pub fn new() -> Self {
        let (reg_tx, reg_rx): (Sender<ServiceMsg>, Receiver<ServiceMsg>) = mpsc::channel();
        let state: Arc<Mutex<SlotMap<TimerState>>> = Arc::new(Mutex::new(SlotMap::new()));

        Self {
            client: TimerClient { reg_tx, state },
            reg_rx,
            params: HashMap::new(),
            groups: HashMap::new(),
        }
    }

    /// Called every frame on the main thread, dispatches expired callbacks.
    pub fn tick(&mut self) {
        while let Ok(msg) = self.reg_rx.try_recv() {
            match msg {
                ServiceMsg::Register(msg) => {
                    let id = msg.id;
                    if let Some(g) = msg.group_id {
                        self.groups.entry(g).or_default().push(id);
                    }
                    let exec = msg.params.exec_immediately;
                    self.params.insert(id, msg.params);
                    if exec {
                        if let Some(p) = self.params.get_mut(&id) {
                            (p.callback)();
                        }
                    }
                }
                ServiceMsg::Destroy(id) => {
                    self.params.remove(&id);
                    for ids in self.groups.values_mut() {
                        ids.retain(|&member| member != id);
                    }
                }
                ServiceMsg::DestroyGroup(group) => {
                    if let Some(ids) = self.groups.remove(&group) {
                        for id in ids {
                            self.params.remove(&id);
                        }
                    }
                }
            }
        }

        let now = Instant::now();
        let fired: Vec<TimerId> = {
            let s = self.client.state.lock().unwrap_or_else(|e| e.into_inner());
            self.params
                .keys()
                .copied()
                .filter(|&id| {
                    s.get(id).is_some_and(|st| {
                        st.alive && !st.paused && st.deadline.is_some_and(|d| now >= d)
                    })
                })
                .collect()
        };

        for id in fired {
            let ok = self
                .client
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(id)
                .is_some_and(|st| st.alive && !st.paused);
            if !ok {
                continue;
            }

            if let Some(p) = self.params.get_mut(&id) {
                (p.callback)();
                if let Some(period) = p.interval {
                    let mut s = self.client.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(st) = s.get_mut(id) {
                        st.remaining = period;
                        st.deadline = Some(Instant::now() + period);
                    }
                } else {
                    self.client
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(id);
                    self.params.remove(&id);
                }
            }
        }
    }
}

impl Default for TimerService {
    fn default() -> Self {
        Self::new()
    }
}

/// TimerClient + auto id cache + Drop auto batch destroy.
///
/// UI base class holds this type; creating via `create_timer` auto-records id,
/// Drop fully cleans up.
pub struct TimerSet {
    client: TimerClient,
    ids: Vec<u32>,
}

impl TimerSet {
    pub fn new(client: TimerClient) -> Self {
        Self {
            client,
            ids: Vec::new(),
        }
    }

    pub fn create_timer(&mut self, params: TimerParams) -> u32 {
        let id = self.client.create_timer(params);
        self.ids.push(id);
        id
    }

    pub fn destroy(&mut self, id: u32) {
        self.client.destroy(id);
        self.ids.retain(|&i| i != id);
    }

    pub fn clear(&mut self) {
        for &id in &self.ids {
            self.client.destroy(id);
        }
        self.ids.clear();
    }

    pub fn pause(&self, id: u32) {
        self.client.pause(id);
    }
    pub fn resume(&self, id: u32) {
        self.client.resume(id);
    }
    pub fn inner(&self) -> &TimerClient {
        &self.client
    }
    pub fn inner_mut(&mut self) -> &mut TimerClient {
        &mut self.client
    }
}

impl Drop for TimerSet {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn timer(flag: Arc<AtomicBool>, exec_immediately: bool) -> TimerParams {
        TimerParams {
            delay: Duration::from_secs(30),
            interval: None,
            exec_immediately,
            group_id: None,
            callback: Box::new(move || flag.store(true, Ordering::SeqCst)),
        }
    }

    #[test]
    fn reused_slot_does_not_fire_destroyed_callback() {
        let mut service = TimerService::new();
        let old_fired = Arc::new(AtomicBool::new(false));
        let new_fired = Arc::new(AtomicBool::new(false));

        let old_id = service.client.create_timer(timer(old_fired.clone(), false));
        service.tick();

        service.client.destroy(old_id);
        let new_id = service.client.create_timer(timer(new_fired.clone(), true));
        assert_eq!(old_id, new_id, "destroyed slot should be reused");

        service.tick();

        assert!(new_fired.load(Ordering::SeqCst), "new callback must fire");
        assert!(
            !old_fired.load(Ordering::SeqCst),
            "destroyed callback must not fire for the reused id"
        );
    }
}
