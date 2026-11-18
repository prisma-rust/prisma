//! Background job pool for project code.
//!
//! Streaming worlds, procedural generation and asset cooking all want to run
//! bounded work off the frame thread: the main thread submits, keeps drawing,
//! and collects finished results later. This module is that pool. It is
//! deliberately small — a priority queue, a fixed set of workers, and a result
//! mailbox — so it adds no dependency to the gameplay layer and works in both
//! builds: the static Player and the editor's `preview-module` cdylib.
//!
//! ## Contract
//!
//! - `try_submit` never blocks. It returns `false` when the pool is already
//!   holding `max_in_flight` jobs, which is how a caller paces generation
//!   without building an unbounded backlog.
//! - Jobs run in ascending `priority` order; equal priorities run in
//!   submission order. Streaming passes a distance-derived priority so the
//!   chunk under the player is generated before the ones at the horizon.
//! - `drain` is the only way results leave the pool. Call it once per frame
//!   into a scratch `Vec` and handle what arrives; results that are never
//!   drained accumulate in the mailbox.
//! - Dropping the pool wakes and joins every worker. Jobs still queued at that
//!   point are discarded, so a shutdown never runs a job against state the
//!   owner has already released.
//!
//! ## Example
//!
//! ```no_run
//! use prism_game_api::jobs::JobPool;
//!
//! let pool: JobPool<u32> = JobPool::new();
//! let mut done = Vec::new();
//! if pool.try_submit(0, || 42) {
//!     // ... keep working until the result is useful ...
//! }
//! pool.drain(&mut done);
//! ```

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// Upper bound on worker threads, regardless of how many cores the machine has.
///
/// Background work here is memory-bandwidth bound (voxel payloads, terrain
/// noise), so a pool wider than this stops paying for itself and starts
/// stealing cycles from the render thread.
const MAX_WORKERS: usize = 8;

/// How many jobs each worker may hold in flight before `try_submit` refuses.
const JOBS_PER_WORKER: usize = 4;

struct Queued<T> {
    priority: u64,
    sequence: u64,
    job: Box<dyn FnOnce() -> T + Send + 'static>,
}

impl<T> PartialEq for Queued<T> {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl<T> Eq for Queued<T> {}

impl<T> PartialOrd for Queued<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Queued<T> {
    /// `BinaryHeap` is a max-heap, so the comparison is inverted: the lowest
    /// priority value (and, within one priority, the earliest submission) is
    /// the greatest element and pops first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

struct State<T> {
    queue: BinaryHeap<Queued<T>>,
    next_sequence: u64,
    shutdown: bool,
}

/// A fixed set of worker threads running priority-ordered jobs and reporting
/// their results through a mailbox.
///
/// `T` is the job result type. One pool serves one result shape; a project that
/// needs several (say, generated chunks and loaded audio) creates one pool per
/// shape rather than wrapping everything in an enum.
pub struct JobPool<T> {
    state: Arc<(Mutex<State<T>>, Condvar)>,
    results: Arc<Mutex<VecDeque<T>>>,
    in_flight: Arc<AtomicUsize>,
    workers: usize,
    max_in_flight: usize,
    handles: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> Default for JobPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> JobPool<T> {
    /// Creates a pool sized from the machine's core count.
    ///
    /// One core is left to the frame thread, so a four-core machine gets three
    /// workers and a single-core machine still gets one.
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
        let workers = cores.saturating_sub(1).clamp(1, MAX_WORKERS);
        Self::with_capacity(workers, workers * JOBS_PER_WORKER)
    }

    /// Creates a pool with an explicit worker count and backlog limit.
    ///
    /// `workers` is clamped to at least one and at most [`MAX_WORKERS`];
    /// `max_in_flight` is clamped to at least one.
    pub fn with_capacity(workers: usize, max_in_flight: usize) -> Self {
        let workers = workers.clamp(1, MAX_WORKERS);
        let max_in_flight = max_in_flight.max(1);
        let state = Arc::new((
            Mutex::new(State {
                queue: BinaryHeap::new(),
                next_sequence: 0,
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let results: Arc<Mutex<VecDeque<T>>> = Arc::new(Mutex::new(VecDeque::new()));
        let in_flight = Arc::new(AtomicUsize::new(0));

        let handles = (0..workers)
            .map(|index| {
                let state = Arc::clone(&state);
                let results = Arc::clone(&results);
                let in_flight = Arc::clone(&in_flight);
                std::thread::Builder::new()
                    .name(format!("prism-job-{index}"))
                    .spawn(move || worker_loop(&state, &results, &in_flight))
                    .expect("job pool worker thread must spawn")
            })
            .collect();

        Self {
            state,
            results,
            in_flight,
            workers,
            max_in_flight,
            handles,
        }
    }

    /// Number of worker threads.
    pub fn worker_count(&self) -> usize {
        self.workers
    }

    /// Jobs this pool will hold (queued plus running) before refusing more.
    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight
    }

    /// Jobs currently queued or running.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(AtomicOrdering::Relaxed)
    }

    /// Whether the backlog is full, i.e. `try_submit` would refuse right now.
    pub fn is_saturated(&self) -> bool {
        self.in_flight() >= self.max_in_flight
    }

    /// Queues `job`, returning `false` when the pool is already saturated.
    ///
    /// Lower `priority` values run first. This never blocks.
    pub fn try_submit<F>(&self, priority: u64, job: F) -> bool
    where
        F: FnOnce() -> T + Send + 'static,
    {
        if self.in_flight.fetch_add(1, AtomicOrdering::AcqRel) >= self.max_in_flight {
            self.in_flight.fetch_sub(1, AtomicOrdering::AcqRel);
            return false;
        }
        let mut state = self.state.0.lock().expect("job pool state poisoned");
        if state.shutdown {
            drop(state);
            self.in_flight.fetch_sub(1, AtomicOrdering::AcqRel);
            return false;
        }
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        state.queue.push(Queued {
            priority,
            sequence,
            job: Box::new(job),
        });
        drop(state);
        self.state.1.notify_one();
        true
    }

    /// Moves every finished result into `out` and returns how many arrived.
    ///
    /// Results are appended in completion order, which is not the submission
    /// order: callers that care about ordering tag the result itself.
    pub fn drain(&self, out: &mut Vec<T>) -> usize {
        let mut results = self.results.lock().expect("job pool results poisoned");
        let count = results.len();
        out.extend(results.drain(..));
        count
    }

    /// Discards every queued job that has not started yet.
    ///
    /// Jobs already running finish normally and still deliver their result.
    pub fn clear_queue(&self) -> usize {
        let mut state = self.state.0.lock().expect("job pool state poisoned");
        let queued = state.queue.len();
        state.queue.clear();
        drop(state);
        self.in_flight.fetch_sub(queued, AtomicOrdering::AcqRel);
        queued
    }
}

impl<T> Drop for JobPool<T> {
    fn drop(&mut self) {
        {
            let mut state = self.state.0.lock().expect("job pool state poisoned");
            state.shutdown = true;
            state.queue.clear();
        }
        self.state.1.notify_all();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_loop<T: Send + 'static>(
    state: &Arc<(Mutex<State<T>>, Condvar)>,
    results: &Arc<Mutex<VecDeque<T>>>,
    in_flight: &Arc<AtomicUsize>,
) {
    loop {
        let job = {
            let mut guard = state.0.lock().expect("job pool state poisoned");
            loop {
                if guard.shutdown {
                    return;
                }
                if let Some(queued) = guard.queue.pop() {
                    break queued;
                }
                guard = state.1.wait(guard).expect("job pool state poisoned");
            }
        };
        let value = (job.job)();
        {
            let mut slot = results.lock().expect("job pool results poisoned");
            slot.push_back(value);
        }
        in_flight.fetch_sub(1, AtomicOrdering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Spins until `pool.drain` has delivered `expected` results.
    fn collect(pool: &JobPool<u32>, expected: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while out.len() < expected && Instant::now() < deadline {
            pool.drain(&mut out);
            if out.len() < expected {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        out
    }

    #[test]
    fn runs_jobs_and_reports_results() {
        let pool: JobPool<u32> = JobPool::with_capacity(2, 8);
        assert_eq!(pool.worker_count(), 2);
        for value in 1..=8u32 {
            assert!(pool.try_submit(0, move || value * value));
        }
        let mut results = collect(&pool, 8);
        results.sort_unstable();
        assert_eq!(results, vec![1, 4, 9, 16, 25, 36, 49, 64]);
        assert_eq!(pool.in_flight(), 0);
    }

    #[test]
    fn refuses_work_beyond_the_backlog_limit() {
        let pool: JobPool<u32> = JobPool::with_capacity(1, 1);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        assert!(pool.try_submit(0, move || {
            let _ = release_rx.recv();
            1
        }));
        // The single slot is taken by the running job.
        assert!(pool.is_saturated());
        assert!(!pool.try_submit(0, || 2));

        release_tx.send(()).expect("release the blocking job");
        let results = collect(&pool, 1);
        assert_eq!(results, vec![1]);
        // Capacity is released once the job finishes.
        assert!(!pool.is_saturated());
        assert!(pool.try_submit(0, || 3));
    }

    #[test]
    fn runs_lower_priority_values_first() {
        // One worker plus a gate makes the ordering deterministic: every job
        // queues while the gate is held, then they pop in priority order.
        let pool: JobPool<u32> = JobPool::with_capacity(1, 8);
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        assert!(pool.try_submit(0, move || {
            let _ = started_tx.send(());
            let _ = gate_rx.recv();
            0
        }));
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("gate job must start");

        assert!(pool.try_submit(30, || 30));
        assert!(pool.try_submit(10, || 10));
        assert!(pool.try_submit(20, || 20));
        gate_tx.send(()).expect("release the gate");

        let results = collect(&pool, 4);
        assert_eq!(results, vec![0, 10, 20, 30]);
    }

    #[test]
    fn dropping_the_pool_discards_queued_jobs() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let ran = Arc::new(AtomicUsize::new(0));
        {
            let pool: JobPool<u32> = JobPool::with_capacity(1, 8);
            let ran_first = Arc::clone(&ran);
            assert!(pool.try_submit(0, move || {
                let _ = started_tx.send(());
                let _ = gate_rx.recv();
                ran_first.fetch_add(1, AtomicOrdering::AcqRel);
                0
            }));
            started_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("blocking job must start");

            let ran_queued = Arc::clone(&ran);
            assert!(pool.try_submit(0, move || {
                ran_queued.fetch_add(1, AtomicOrdering::AcqRel);
                1
            }));
            gate_tx.send(()).expect("release the blocking job");
            // Dropping joins the worker, so the running job still completes.
        }
        assert_eq!(
            ran.load(AtomicOrdering::Acquire),
            1,
            "only the running job may execute; the queued one is discarded"
        );
    }
}
