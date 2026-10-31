//! Task queue, priorities, schedules, pause/resume and cancellation.
//!
//! One worker thread per scheduler owns the device it was created with; jobs run
//! one at a time, in `(due time, priority, submission order)` order.

use crate::device::Device;
use crate::error::{Error, Result};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub type TaskId = u64;

/// Higher runs first when two jobs are due at the same time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(pub i32);

const MAX_ERRORS: usize = 32;

type OnceFn = Box<dyn FnOnce(&Device) -> Result<()> + Send>;
type EveryFn = Arc<dyn Fn(&Device) -> Result<()> + Send + Sync>;

enum Action {
    Once(Option<OnceFn>),
    Every(EveryFn),
}

struct Job {
    id: TaskId,
    due_ms: u64,
    priority: Priority,
    seq: u64,
    every: Option<u64>,
    action: Action,
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for Job {}
impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: reverse so the earliest, highest-priority
        // job is popped first.
        other
            .due_ms
            .cmp(&self.due_ms)
            .then(self.priority.cmp(&other.priority))
            .then(other.seq.cmp(&self.seq))
    }
}

enum Msg {
    Submit(Job),
    Cancel(TaskId),
    Pause(bool),
    Wait(Sender<()>),
    Shutdown,
}

struct Shared {
    started: Instant,
    tx: Sender<Msg>,
    paused: Arc<AtomicBool>,
    next_id: AtomicU64,
    next_seq: AtomicU64,
    errors: Arc<Mutex<Vec<(TaskId, Error)>>>,
    cancelled: Arc<Mutex<HashSet<TaskId>>>,
    join: Mutex<Option<JoinHandle<()>>>,
    shutdown: AtomicBool,
}

pub struct Scheduler {
    shared: Arc<Shared>,
}

impl Scheduler {
    pub fn new(device: Device) -> Self {
        let (tx, rx) = channel();
        let paused = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();
        let shared = Arc::new(Shared {
            started,
            tx,
            paused: paused.clone(),
            next_id: AtomicU64::new(1),
            next_seq: AtomicU64::new(1),
            errors: errors.clone(),
            cancelled: Arc::new(Mutex::new(HashSet::new())),
            join: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });
        let worker_device = device.clone();
        let cancelled = shared.cancelled.clone();
        let handle =
            thread::spawn(move || worker(started, worker_device, rx, paused, errors, cancelled));
        *shared.join.lock().unwrap() = Some(handle);
        Self { shared }
    }

    pub fn submit<F>(&self, delay: Duration, priority: Priority, action: F) -> Result<TaskId>
    where
        F: FnOnce(&Device) -> Result<()> + Send + 'static,
    {
        let id = self.shared.next_id.fetch_add(1, AtomicOrdering::Relaxed);
        let seq = self.shared.next_seq.fetch_add(1, AtomicOrdering::Relaxed);
        let job = Job {
            id,
            due_ms: self.due_ms(delay),
            priority,
            seq,
            every: None,
            action: Action::Once(Some(Box::new(action))),
        };
        self.shared
            .tx
            .send(Msg::Submit(job))
            .map_err(|_| Error::io("scheduler is shut down"))?;
        Ok(id)
    }

    /// Repeats until cancelled. The first run happens after `interval`.
    pub fn every<F>(&self, interval: Duration, priority: Priority, action: F) -> Result<TaskId>
    where
        F: Fn(&Device) -> Result<()> + Send + Sync + 'static,
    {
        if interval.is_zero() {
            return Err(Error::invalid("interval must be positive"));
        }
        let id = self.shared.next_id.fetch_add(1, AtomicOrdering::Relaxed);
        let seq = self.shared.next_seq.fetch_add(1, AtomicOrdering::Relaxed);
        let job = Job {
            id,
            due_ms: self.due_ms(interval),
            priority,
            seq,
            every: Some(interval.as_millis() as u64),
            action: Action::Every(Arc::new(action)),
        };
        self.shared
            .tx
            .send(Msg::Submit(job))
            .map_err(|_| Error::io("scheduler is shut down"))?;
        Ok(id)
    }

    pub fn cancel(&self, id: TaskId) {
        // Inserting here (not only in the worker) means a cancel that races a
        // due job still wins as long as the callback has not started.
        self.shared.cancelled.lock().unwrap().insert(id);
        let _ = self.shared.tx.send(Msg::Cancel(id));
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, AtomicOrdering::Relaxed);
        let _ = self.shared.tx.send(Msg::Pause(true));
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, AtomicOrdering::Relaxed);
        let _ = self.shared.tx.send(Msg::Pause(false));
    }

    /// Blocks until the queue is empty and no job is running.
    pub fn wait_idle(&self, timeout: Duration) -> Result<()> {
        let (tx, rx) = channel();
        self.shared
            .tx
            .send(Msg::Wait(tx))
            .map_err(|_| Error::io("scheduler is shut down"))?;
        match rx.recv_timeout(timeout) {
            Ok(()) => Ok(()),
            Err(_) => Err(Error::timeout("scheduler did not become idle")),
        }
    }

    fn due_ms(&self, delay: Duration) -> u64 {
        self.shared.started.elapsed().as_millis() as u64 + delay.as_millis() as u64
    }

    pub fn take_errors(&self) -> Vec<(TaskId, Error)> {
        std::mem::take(&mut *self.shared.errors.lock().unwrap())
    }

    pub fn shutdown(&self) {
        if self.shared.shutdown.swap(true, AtomicOrdering::Relaxed) {
            return;
        }
        let _ = self.shared.tx.send(Msg::Shutdown);
        if let Some(handle) = self.shared.join.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker(
    started: Instant,
    device: Device,
    rx: Receiver<Msg>,
    paused: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<(TaskId, Error)>>>,
    cancelled: Arc<Mutex<HashSet<TaskId>>>,
) {
    let mut heap: BinaryHeap<Job> = BinaryHeap::new();
    let mut waiters: Vec<Sender<()>> = Vec::new();
    loop {
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let timeout = if paused.load(AtomicOrdering::Relaxed) {
            Duration::from_millis(20)
        } else {
            heap.peek()
                .map(|job| {
                    Duration::from_millis(job.due_ms.saturating_sub(elapsed_ms).clamp(1, 20))
                })
                .unwrap_or(Duration::from_millis(20))
        };
        match rx.recv_timeout(timeout) {
            Ok(Msg::Submit(job)) => heap.push(job),
            Ok(Msg::Cancel(id)) => {
                cancelled.lock().unwrap().insert(id);
            }
            Ok(Msg::Pause(value)) => paused.store(value, AtomicOrdering::Relaxed),
            Ok(Msg::Wait(reply)) => waiters.push(reply),
            Ok(Msg::Shutdown) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !paused.load(AtomicOrdering::Relaxed) {
            let now = started.elapsed().as_millis() as u64;
            while let Some(job) = heap.peek() {
                if job.due_ms > now {
                    break;
                }
                let mut job = heap.pop().expect("peeked job");
                if cancelled.lock().unwrap().remove(&job.id) {
                    continue;
                }
                let result = match &mut job.action {
                    Action::Once(action) => match action.take() {
                        Some(action) => action(&device),
                        None => Ok(()),
                    },
                    Action::Every(action) => action(&device),
                };
                if let Err(error) = result {
                    let mut slot = errors.lock().unwrap();
                    if slot.len() >= MAX_ERRORS {
                        slot.remove(0);
                    }
                    slot.push((job.id, error));
                }
                if let Some(interval) = job.every {
                    if cancelled.lock().unwrap().remove(&job.id) {
                        continue;
                    }
                    let mut next = job.due_ms + interval;
                    let now = started.elapsed().as_millis() as u64;
                    while next <= now {
                        next += interval;
                    }
                    job.due_ms = next;
                    heap.push(job);
                }
            }
        }
        if heap.is_empty() {
            for waiter in waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
    }
    for waiter in waiters.drain(..) {
        let _ = waiter.send(());
    }
    let _ = device;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceOptions;
    use crate::manager::Manager;
    use std::sync::Mutex;

    fn device() -> (Device, crate::mock::MockHandle) {
        let (device, handle) = Manager::open_mock_with_handle(DeviceOptions::default()).unwrap();
        (device, handle)
    }

    #[test]
    fn runs_by_due_time_then_priority_then_submission_order() {
        let (device, _handle) = device();
        let scheduler = Scheduler::new(device.clone());
        // Queue everything before the worker is allowed to run so the priority
        // tie-break is deterministic.
        scheduler.pause();
        let order = Arc::new(Mutex::new(Vec::new()));
        let record = |name: &'static str, order: Arc<Mutex<Vec<&'static str>>>| {
            move |_device: &Device| {
                order.lock().unwrap().push(name);
                Ok(())
            }
        };
        scheduler
            .submit(
                Duration::from_millis(0),
                Priority(0),
                record("low", order.clone()),
            )
            .unwrap();
        scheduler
            .submit(
                Duration::from_millis(0),
                Priority(10),
                record("high", order.clone()),
            )
            .unwrap();
        scheduler
            .submit(
                Duration::from_millis(0),
                Priority(0),
                record("second", order.clone()),
            )
            .unwrap();
        scheduler.resume();
        scheduler.wait_idle(Duration::from_secs(2)).unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["high", "low", "second"]);
    }

    #[test]
    fn pause_stops_execution_and_resume_continues() {
        let (device, _handle) = device();
        let scheduler = Scheduler::new(device);
        let ran = Arc::new(AtomicBool::new(false));
        scheduler.pause();
        let flag = ran.clone();
        scheduler
            .submit(Duration::from_millis(0), Priority(0), move |_| {
                flag.store(true, AtomicOrdering::Relaxed);
                Ok(())
            })
            .unwrap();
        thread::sleep(Duration::from_millis(80));
        assert!(!ran.load(AtomicOrdering::Relaxed));
        scheduler.resume();
        scheduler.wait_idle(Duration::from_secs(2)).unwrap();
        assert!(ran.load(AtomicOrdering::Relaxed));
    }

    #[test]
    fn cancelled_jobs_never_run() {
        let (device, _handle) = device();
        let scheduler = Scheduler::new(device);
        let ran = Arc::new(AtomicBool::new(false));
        scheduler.pause();
        let flag = ran.clone();
        let id = scheduler
            .submit(Duration::from_millis(0), Priority(0), move |_| {
                flag.store(true, AtomicOrdering::Relaxed);
                Ok(())
            })
            .unwrap();
        scheduler.cancel(id);
        scheduler.resume();
        scheduler.wait_idle(Duration::from_secs(2)).unwrap();
        assert!(!ran.load(AtomicOrdering::Relaxed));
    }

    #[test]
    fn repeating_jobs_run_until_cancelled() {
        let (device, _handle) = device();
        let scheduler = Scheduler::new(device);
        let count = Arc::new(AtomicU64::new(0));
        let counter = count.clone();
        let id = scheduler
            .every(Duration::from_millis(10), Priority(0), move |_| {
                counter.fetch_add(1, AtomicOrdering::Relaxed);
                Ok(())
            })
            .unwrap();
        thread::sleep(Duration::from_millis(80));
        scheduler.cancel(id);
        let seen = count.load(AtomicOrdering::Relaxed);
        assert!(seen >= 2, "ran {seen} times");
        thread::sleep(Duration::from_millis(40));
        let stopped = count.load(AtomicOrdering::Relaxed);
        thread::sleep(Duration::from_millis(40));
        assert_eq!(stopped, count.load(AtomicOrdering::Relaxed));
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        let (device, _handle) = device();
        let scheduler = Scheduler::new(device);
        scheduler
            .submit(Duration::from_millis(0), Priority(0), |_| {
                Err(Error::invalid("boom"))
            })
            .unwrap();
        scheduler.wait_idle(Duration::from_secs(2)).unwrap();
        let errors = scheduler.take_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].1.code(), crate::error::ErrorCode::Invalid);
    }
}
