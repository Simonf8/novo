//! A small thread pool for many short parallel loops in a row, such as the
//! matrix-vector products of generating one token. Idle workers spin briefly
//! before sleeping, so back-to-back loops do not pay to wake them each time.

use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// How long an idle worker keeps checking for work before it sleeps.
const SPIN: u32 = 1 << 16;

type Job = &'static (dyn Fn(usize) + Sync);

struct Shared {
    /// The running loop, if any; cleared before the loop's caller returns.
    job: Mutex<Option<(Job, usize)>>,
    epoch: AtomicU64,
    next: AtomicUsize,
    done: AtomicUsize,
    /// Workers currently holding the job.
    active: AtomicUsize,
    wake: Condvar,
    sleep: Mutex<()>,
}

struct Pool {
    shared: Arc<Shared>,
    /// One loop at a time; concurrent callers queue here.
    running: Mutex<()>,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let shared = Arc::new(Shared {
            job: Mutex::new(None),
            epoch: AtomicU64::new(0),
            next: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            wake: Condvar::new(),
            sleep: Mutex::new(()),
        });
        let workers = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .saturating_sub(1);
        for _ in 0..workers {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("novo-pool".into())
                .spawn(move || work(&shared))
                .expect("spawn a pool worker");
        }
        Pool {
            shared,
            running: Mutex::new(()),
        }
    })
}

fn work(shared: &Shared) {
    let mut seen = 0;
    loop {
        let mut spins = 0;
        while shared.epoch.load(Ordering::Acquire) == seen {
            if spins < SPIN {
                spins += 1;
                std::hint::spin_loop();
            } else {
                let mut guard = shared.sleep.lock();
                if shared.epoch.load(Ordering::Acquire) == seen {
                    shared.wake.wait(&mut guard);
                }
            }
        }
        seen = shared.epoch.load(Ordering::Acquire);
        let job = {
            let job = shared.job.lock();
            if job.is_some() {
                shared.active.fetch_add(1, Ordering::AcqRel);
            }
            *job
        };
        if let Some((f, n)) = job {
            run(shared, f, n);
            shared.active.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

fn run(shared: &Shared, f: Job, n: usize) {
    loop {
        let i = shared.next.fetch_add(1, Ordering::AcqRel);
        if i >= n {
            return;
        }
        f(i);
        shared.done.fetch_add(1, Ordering::AcqRel);
    }
}

/// Calls `f(i)` for every `i` in `0..n`, spread over the pool and this thread.
pub fn for_each(n: usize, f: &(dyn Fn(usize) + Sync)) {
    let p = pool();
    let _one_at_a_time = p.running.lock();
    let shared = &*p.shared;
    // SAFETY: the job is removed and every worker has let go of it before this
    // function returns, so `f` outlives all uses of the extended reference.
    let job: Job = unsafe { std::mem::transmute::<&(dyn Fn(usize) + Sync), Job>(f) };
    shared.next.store(0, Ordering::Release);
    shared.done.store(0, Ordering::Release);
    *shared.job.lock() = Some((job, n));
    {
        let _guard = shared.sleep.lock();
        shared.epoch.fetch_add(1, Ordering::AcqRel);
        shared.wake.notify_all();
    }
    run(shared, job, n);
    while shared.done.load(Ordering::Acquire) < n {
        std::hint::spin_loop();
    }
    *shared.job.lock() = None;
    while shared.active.load(Ordering::Acquire) > 0 {
        std::hint::spin_loop();
    }
}
