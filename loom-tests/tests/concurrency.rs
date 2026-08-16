//! Loom model-checking tests for the concurrency patterns NeoCoder relies on.
//!
//! Loom explores *all* thread interleavings of a small program and verifies
//! that assertions hold under every schedule — catching lost wakeups, torn
//! reads and ordering bugs that never reproduce under normal testing.
//!
//! Run with: `RUSTFLAGS="--cfg loom" cargo test -p loom-tests`
//!
//! The tests model the handshake patterns used by the agent runtime:
//!   1. the pause/resume protocol (`PauseControl`: flag + notify, waiters park
//!      on the flag and are woken by the notifier), and
//!   2. the session registry pattern (shared map keyed by session id).

#[cfg(loom)]
mod loom_tests {
    use loom::sync::atomic::{AtomicBool, Ordering};
    use loom::sync::{Arc, Condvar, Mutex};

    /// The pause/resume handshake: a waiter parks until the flag is set, and
    /// the notifier must never lose a wakeup regardless of scheduling.
    #[test]
    fn pause_resume_never_loses_wakeup() {
        loom::model(|| {
            let paused = Arc::new(AtomicBool::new(false));
            let lock = Arc::new(Mutex::new(()));
            let cv = Arc::new(Condvar::new());

            let (p2, l2, cv2) = (paused.clone(), lock.clone(), cv.clone());

            let waiter = loom::thread::spawn(move || {
                let mut guard = l2.lock().unwrap();
                // Park while paused; the loop handles spurious wakeups.
                while !p2.load(Ordering::SeqCst) {
                    guard = cv2.wait(guard).unwrap();
                }
                assert!(p2.load(Ordering::SeqCst), "waiter observed unpaused state");
            });

            // Resume: publish the flag first, then notify — this ordering
            // guarantees the waiter cannot miss the wakeup.
            paused.store(true, Ordering::SeqCst);
            {
                let _g = lock.lock().unwrap();
                cv.notify_all();
            }

            waiter.join().unwrap();
        });
    }

    /// Two waiters + one notifier: a single `notify_all` must release both.
    #[test]
    fn notify_all_releases_all_waiters() {
        loom::model(|| {
            let go = Arc::new(AtomicBool::new(false));
            let lock = Arc::new(Mutex::new(()));
            let cv = Arc::new(Condvar::new());

            let mut waiters = Vec::new();
            for _ in 0..2 {
                let (g, l, c) = (go.clone(), lock.clone(), cv.clone());
                waiters.push(loom::thread::spawn(move || {
                    let mut guard = l.lock().unwrap();
                    while !g.load(Ordering::SeqCst) {
                        guard = c.wait(guard).unwrap();
                    }
                }));
            }

            go.store(true, Ordering::SeqCst);
            {
                let _g = lock.lock().unwrap();
                cv.notify_all();
            }

            for w in waiters {
                w.join().unwrap();
            }
        });
    }

    /// Session-registry pattern: concurrent inserts of distinct keys must all
    /// be visible and none lost (models `PauseControl`'s HashMap keyed by
    /// session id).
    #[test]
    fn session_registry_no_lost_inserts() {
        loom::model(|| {
            const N: usize = 2;

            let map = Arc::new(Mutex::new(std::collections::HashMap::<u32, u32>::new()));

            let mut writers = Vec::new();
            for i in 0..N {
                let m = map.clone();
                writers.push(loom::thread::spawn(move || {
                    let mut g = m.lock().unwrap();
                    g.insert(i as u32, i as u32);
                }));
            }
            for w in writers {
                w.join().unwrap();
            }

            let g = map.lock().unwrap();
            assert_eq!(g.len(), N, "every session insert must be visible");
            for i in 0..N as u32 {
                assert_eq!(g.get(&i), Some(&i), "session {i} missing");
            }
        });
    }

    /// Counter increments under contention: 2 threads × 3 increments must
    /// total 6 exactly (no torn updates). Kept small — loom enumerates every
    /// interleaving, and the schedule space grows combinatorially.
    #[test]
    fn counter_increments_are_not_lost() {
        loom::model(|| {
            let counter = Arc::new(Mutex::new(0usize));
            let mut threads = Vec::new();
            for _ in 0..2 {
                let c = counter.clone();
                threads.push(loom::thread::spawn(move || {
                    for _ in 0..3 {
                        let mut g = c.lock().unwrap();
                        *g += 1;
                    }
                }));
            }
            for t in threads {
                t.join().unwrap();
            }
            let g = counter.lock().unwrap();
            assert_eq!(*g, 6, "no increment may be lost");
        });
    }
}

// Without `--cfg loom` the file compiles to an empty harness so plain
// `cargo test` keeps passing (loom requires nightly to explore schedules).
#[cfg(not(loom))]
mod loom_tests {
    #[test]
    fn loom_not_enabled() {
        // Placeholder: real model checking runs with
        //   RUSTFLAGS="--cfg loom" cargo test -p loom-tests
    }
}
