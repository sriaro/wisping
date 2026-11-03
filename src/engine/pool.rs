use std::collections::VecDeque;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use super::lock::LockExt;

type Job = Box<dyn FnOnce() + Send + 'static>;

struct Queue {
    jobs: VecDeque<Job>,
    idle: usize,
    starting: usize,
    workers: usize,
}

const MAX_WORKERS: usize = 32;

static QUEUE: Mutex<Queue> = Mutex::new(Queue { jobs: VecDeque::new(), idle: 0, starting: 0, workers: 0 });
static READY: Condvar = Condvar::new();

fn push(job: Job) -> std::io::Result<()> {
    let mut q = QUEUE.locked();
    if q.jobs.len() >= q.idle + q.starting && q.workers + q.starting < MAX_WORKERS {
        q.starting += 1;
        if let Err(error) = std::thread::Builder::new().name("wisping-worker".into()).spawn(worker) {
            q.starting -= 1;
            return Err(error);
        }
        q.workers += 1;
    }
    q.jobs.push_back(job);
    drop(q);
    READY.notify_one();
    Ok(())
}

fn worker() {
    let mut q = QUEUE.locked();
    q.starting -= 1;
    loop {
        if let Some(job) = q.jobs.pop_front() {
            drop(q);
            let _ = catch_unwind(AssertUnwindSafe(job));
            super::cpu::normal();
            q = QUEUE.locked();
            continue;
        }
        q.idle += 1;
        q = READY.wait(q).unwrap_or_else(PoisonError::into_inner);
        q.idle -= 1;
    }
}

pub fn spawn(f: impl FnOnce() + Send + 'static) {
    if let Err(e) = push(Box::new(f)) {
        panic!("failed to spawn thread: {e}");
    }
}

struct Slot<T> {
    value: Mutex<Option<std::thread::Result<T>>>,
    ready: Condvar,
}

impl<T> Slot<T> {
    fn new() -> Arc<Self> {
        Arc::new(Slot { value: Mutex::new(None), ready: Condvar::new() })
    }

    fn put(&self, result: std::thread::Result<T>) {
        *self.value.locked() = Some(result);
        self.ready.notify_all();
    }

    fn take(&self) -> std::thread::Result<T> {
        let mut value = self.value.locked();
        loop {
            if let Some(result) = value.take() {
                return result;
            }
            value = self.ready.wait(value).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

pub struct Task<T>(Arc<Slot<T>>);

impl<T> Task<T> {
    pub fn join(self) -> std::thread::Result<T> {
        self.0.take()
    }
}

pub fn run<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Task<T> {
    let slot = Slot::new();
    let out = slot.clone();
    spawn(move || out.put(catch_unwind(AssertUnwindSafe(f))));
    Task(slot)
}

struct ScopeState {
    pending: Mutex<usize>,
    done: Condvar,
    lost: AtomicUsize,
}

struct Finish(Arc<ScopeState>);

impl Drop for Finish {
    fn drop(&mut self) {
        let mut pending = self.0.pending.locked();
        *pending -= 1;
        if *pending == 0 {
            self.0.done.notify_all();
        }
    }
}

pub struct Scope<'scope, 'env: 'scope> {
    state: Arc<ScopeState>,
    scope: PhantomData<&'scope mut &'scope ()>,
    env: PhantomData<&'env mut &'env ()>,
}

pub struct ScopedJoinHandle<'scope, T> {
    slot: Arc<Slot<T>>,
    state: Arc<ScopeState>,
    scope: PhantomData<&'scope ()>,
}

impl<'scope> Scope<'scope, '_> {
    pub fn spawn<F, T>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let slot = Slot::new();
        let out = slot.clone();
        let state = self.state.clone();
        let job: Box<dyn FnOnce() + Send + 'scope> = Box::new(move || {
            let finish = Finish(state);
            let out = out;
            let result = catch_unwind(AssertUnwindSafe(f));
            if result.is_err() {
                finish.0.lost.fetch_add(1, Ordering::SeqCst);
            }
            out.put(result);
        });
        let job: Job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'scope>, Job>(job) };
        *self.state.pending.locked() += 1;
        if let Err(e) = push(job) {
            drop(Finish(self.state.clone()));
            panic!("failed to spawn thread: {e}");
        }
        ScopedJoinHandle { slot, state: self.state.clone(), scope: PhantomData }
    }
}

impl<T> ScopedJoinHandle<'_, T> {
    pub fn join(self) -> std::thread::Result<T> {
        let result = self.slot.take();
        if result.is_err() {
            self.state.lost.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }
}

pub fn scope<'env, F, T>(f: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let scope = Scope { state: Arc::new(ScopeState { pending: Mutex::new(0), done: Condvar::new(), lost: AtomicUsize::new(0) }), scope: PhantomData, env: PhantomData };
    let result = catch_unwind(AssertUnwindSafe(|| f(&scope)));
    let mut pending = scope.state.pending.locked();
    while *pending > 0 {
        pending = scope.state.done.wait(pending).unwrap_or_else(PoisonError::into_inner);
    }
    drop(pending);
    match result {
        Err(payload) => resume_unwind(payload),
        Ok(_) if scope.state.lost.load(Ordering::SeqCst) > 0 => panic!("a scoped thread panicked"),
        Ok(value) => value,
    }
}
