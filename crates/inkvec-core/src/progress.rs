//! Live progress and cooperative cancellation for one trace.
//!
//! A trace of a large logo takes seconds, some of them tens of seconds, and nearly all of that
//! time is spent in a handful of loops: the palette's gradient-band merge, the boundary solve's
//! iterations, the curve fit over every ring, the crossing repair. An interface watching the
//! trace needs two things from those loops, and this module is the seam for both:
//!
//! - **What is happening now.** [`begin`] names the stage that has just started, [`step`] and
//!   [`Handle::tick`] say how far through its loop it is, and [`note`] records a fact worth a
//!   line in a log ("8 inks"). The stage boundaries the [`Stopwatch`](../../inkvec_trace/struct.Stopwatch.html)
//!   already marks are recorded here too, through [`end`].
//! - **The right to stop it.** [`Progress::cancel`] asks the trace to stop. Every one of the
//!   calls above is also a cancellation point: once cancelled, the next one unwinds the trace
//!   with a [`Cancelled`] payload, which the embedder catches (`std::panic::catch_unwind`) and
//!   reads as "stopped on request" rather than as a failure. The unwind is raised with
//!   `std::panic::resume_unwind`, so no panic hook runs and nothing is printed.
//!
//! None of it changes what a trace computes. With no [`Progress`] installed on the thread
//! (the command line, the library, every binding) each call is a thread-local read and a
//! branch; with one installed it is also an atomic or two and, for the rare journal entry, a
//! short lock. The output is byte-identical either way.
//!
//! The progress is installed per thread by [`with`], like the stage sink, because the pipeline
//! is driven from one thread. Its parallel loops run on rayon's workers, which cannot see that
//! thread's locals, so a loop takes a [`Handle`] before it fans out and ticks through that.
//! A handle is an explicit capture rather than an ambient one on purpose: a worker that steals
//! a job belonging to somebody else's trace must not find this trace's cancellation there.

use crate::clock::Instant;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The payload a cancelled trace unwinds with. See [`is_cancelled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// Whether an unwind payload (from `std::panic::catch_unwind`) is a cancellation rather than
/// a real panic.
pub fn is_cancelled(payload: &(dyn std::any::Any + Send)) -> bool {
    payload.is::<Cancelled>()
}

/// One thing the pipeline reported, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A stage started. The name is the pipeline's internal one (`merge_bands`, `fit_dp`).
    Begin {
        /// The stage.
        stage: &'static str,
        /// Milliseconds since the progress was created.
        at_ms: f64,
    },
    /// A stage finished, as the stopwatch marked it.
    End {
        /// The stage, by its internal name.
        stage: String,
        /// How long it took, in milliseconds.
        ms: f64,
        /// Milliseconds since the progress was created.
        at_ms: f64,
    },
    /// A fact about the stage that is running, worth a line in a log.
    Note {
        /// The stage it belongs to: the one last begun, or `""` before any.
        stage: &'static str,
        /// What happened, in a few words.
        text: String,
        /// Milliseconds since the progress was created.
        at_ms: f64,
    },
}

/// How far through its loop the running stage is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The stage the loop belongs to.
    pub stage: &'static str,
    /// What the loop counts: `"rings"`, `"iterations"`, `"rounds"`.
    pub unit: &'static str,
    /// How many are done.
    pub done: u64,
    /// How many there are; 0 when the loop does not know in advance.
    pub total: u64,
}

/// What happened since the last [`Progress::take`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// The journal entries, oldest first.
    pub entries: Vec<Entry>,
    /// The running stage's step, when it moved since the last report.
    pub step: Option<Step>,
}

impl Report {
    /// Nothing to say.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.step.is_none()
    }
}

/// The most journal entries kept for a reader that has not taken them. A trace writes a few
/// dozen; the cap only matters when nobody reads at all.
const JOURNAL_CAP: usize = 2048;

/// One trace's progress, shared between the thread running it, the workers it fans out to,
/// and whoever is watching.
///
/// The loop counters are atomics because rayon's workers tick them concurrently; the rest
/// (stage names and the journal) sits behind one mutex that only stage boundaries and notes
/// take. Relaxed ordering is enough for `cancelled`, `done` and `total`: each is a single
/// value read on its own, and a reader seeing it one tick late is harmless. `moved` is
/// bumped with `Release` after the counters change and read with `Acquire` in
/// [`take`](Self::take), so a report that sees the bump also sees the counts that caused it.
#[derive(Debug)]
pub struct Progress {
    started: Instant,
    cancelled: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    /// Bumped by every change to the step, so a reader can tell whether it moved.
    moved: AtomicU64,
    /// The value of `moved` the last report carried.
    reported: AtomicU64,
    state: Mutex<State>,
}

/// The part of a [`Progress`] that changes rarely and is not a single number.
#[derive(Debug, Default)]
struct State {
    /// The stage last begun, `""` before any.
    stage: &'static str,
    /// What the running stage's loop counts, `""` until it reports a [`step`]. A stage with
    /// no unit reports no [`Step`].
    unit: &'static str,
    /// Entries not yet taken, oldest first, at most [`JOURNAL_CAP`].
    journal: Vec<Entry>,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    /// A fresh progress, its clock starting now.
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            cancelled: AtomicBool::new(false),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            moved: AtomicU64::new(0),
            reported: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }
    }

    /// Ask the trace to stop at its next cancellation point. Safe from any thread, any number
    /// of times.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Milliseconds since this progress was created.
    pub fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1e3
    }

    /// Whether journal entries are waiting to be taken: a stage started or ended, or a
    /// note. A caller that throttles the loop counts still passes these on at once.
    pub fn has_news(&self) -> bool {
        !self.lock().journal.is_empty()
    }

    /// Everything reported since the last call: the journal, drained, and the step if it
    /// moved. Cheap enough to poll every few tens of milliseconds.
    pub fn take(&self) -> Report {
        let moved = self.moved.load(Ordering::Acquire);
        let fresh = self.reported.swap(moved, Ordering::AcqRel) != moved;
        let mut state = self.lock();
        let entries = std::mem::take(&mut state.journal);
        let step = (fresh && !state.unit.is_empty()).then(|| Step {
            stage: state.stage,
            unit: state.unit,
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        });
        Report { entries, step }
    }

    /// The shared state, recovered even if a thread panicked while holding it: a cancelled
    /// trace unwinds through here by design, and the state it leaves is still consistent
    /// (every write under the lock is a single assignment or push).
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Unwind with [`Cancelled`] if the trace has been cancelled.
    fn check(&self) {
        if self.is_cancelled() {
            std::panic::resume_unwind(Box::new(Cancelled));
        }
    }

    /// Record that `stage` started, from outside the pipeline: the embedder's own work
    /// around a trace, such as measuring the result. Never a cancellation point.
    pub fn announce(&self, stage: &'static str) {
        let at_ms = self.elapsed_ms();
        {
            let mut state = self.lock();
            state.stage = stage;
            state.unit = "";
            if state.journal.len() < JOURNAL_CAP {
                state.journal.push(Entry::Begin { stage, at_ms });
            }
        }
        self.done.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
    }

    /// Record that `stage` finished after `ms` milliseconds. Never a cancellation point.
    pub fn finished(&self, stage: &str, ms: f64) {
        self.record(Entry::End {
            stage: stage.to_string(),
            ms,
            at_ms: self.elapsed_ms(),
        });
    }

    /// Append `entry` to the journal, dropping it once [`JOURNAL_CAP`] entries are waiting.
    fn record(&self, entry: Entry) {
        let mut state = self.lock();
        if state.journal.len() < JOURNAL_CAP {
            state.journal.push(entry);
        }
    }
}

/// A reader called on the pipeline's thread whenever something was reported there. For an
/// embedder with no thread to poll from (a browser worker); it should throttle itself.
type Wake = Rc<dyn Fn(&Progress)>;

/// What [`with`] or [`with_wake`] put on the pipeline's thread.
struct Installed {
    progress: Arc<Progress>,
    wake: Option<Wake>,
}

thread_local! {
    static CURRENT: RefCell<Option<Installed>> = const { RefCell::new(None) };
    /// A progress a worker is checking for while it does one piece of a parallel loop's
    /// work: see [`Handle::scoped`]. Cancellation only; nothing is reported through it.
    static WORKER: RefCell<Option<Arc<Progress>>> = const { RefCell::new(None) };
}

/// Run `f` with `progress` receiving this thread's reports and able to cancel it.
///
/// A cancelled trace unwinds out of `f` with a [`Cancelled`] payload; catch it with
/// `std::panic::catch_unwind` and test it with [`is_cancelled`]. Whatever was installed
/// before is put back on every way out, including that one.
pub fn with<R>(progress: Arc<Progress>, f: impl FnOnce() -> R) -> R {
    install(
        Installed {
            progress,
            wake: None,
        },
        f,
    )
}

/// [`with`], also calling `wake` on this thread after each report made on it.
///
/// For an embedder that cannot poll from another thread: a browser worker is busy for the
/// whole trace, so the only chance it gets to pass progress on is when the pipeline hands
/// it one. Reports made on rayon's workers are not seen until the pipeline's own thread
/// reports again.
pub fn with_wake<R>(
    progress: Arc<Progress>,
    wake: impl Fn(&Progress) + 'static,
    f: impl FnOnce() -> R,
) -> R {
    install(
        Installed {
            progress,
            wake: Some(Rc::new(wake)),
        },
        f,
    )
}

/// Put `installed` on this thread for the duration of `f`. The previous occupant is held by
/// a drop guard and restored however `f` leaves, including by unwinding, so nested traces
/// on one thread each see their own progress.
fn install<R>(installed: Installed, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Installed>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.borrow_mut().replace(installed)));
    f()
}

/// The installed progress and its wake, cloned out so neither is borrowed while called.
fn current() -> Option<(Arc<Progress>, Option<Wake>)> {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .map(|i| (Arc::clone(&i.progress), i.wake.clone()))
    })
}

/// Whether a progress is installed on this thread. Lets a caller skip preparing a report
/// nobody will read.
pub fn active() -> bool {
    CURRENT.with(|c| c.borrow().is_some())
}

/// The common path of every pipeline-side report: nothing at all when no progress is
/// installed; otherwise a cancellation check first (so a cancelled trace records nothing
/// more), then `apply`, then the wake callback if there is one.
fn report(apply: impl FnOnce(&Progress)) {
    if let Some((p, wake)) = current() {
        p.check();
        apply(&p);
        if let Some(wake) = wake {
            wake(&p);
        }
    }
}

/// A stage has started. `stage` is the pipeline's internal name for it.
pub fn begin(stage: &'static str) {
    report(|p| p.announce(stage));
}

/// A stage has finished, after `ms` milliseconds. What the stopwatch's mark calls.
pub fn end(stage: &str, ms: f64) {
    report(|p| p.finished(stage, ms));
}

/// Something worth a line in the log, about the stage that is running. `text` is only built
/// when somebody is listening.
pub fn note(text: impl FnOnce() -> String) {
    report(|p| {
        let stage = p.lock().stage;
        p.record(Entry::Note {
            stage,
            text: text(),
            at_ms: p.elapsed_ms(),
        })
    });
}

/// The running stage's loop is at `done` of `total` `unit` (`total` 0 when unknown). Called
/// from the pipeline's own thread; a parallel loop sets its total here and ticks through a
/// [`Handle`].
pub fn step(unit: &'static str, done: u64, total: u64) {
    report(|p| {
        p.lock().unit = unit;
        p.done.store(done, Ordering::Relaxed);
        p.total.store(total, Ordering::Relaxed);
        p.moved.fetch_add(1, Ordering::Release);
    });
}

/// A cancellation point and nothing else, for a long stretch that reports no progress.
///
/// Also reached from a worker inside [`Handle::scoped`], which is what lets code that knows
/// nothing of progress (the curve fitter's dynamic program) stop a long piece of work in the
/// middle.
pub fn checkpoint() {
    if let Some((p, _)) = current() {
        p.check();
    }
    let worker = WORKER.with(|w| w.borrow().clone());
    if let Some(p) = worker {
        p.check();
    }
}

/// Run `f` with no worker scope in force on this thread, then put it back (also on unwind).
///
/// For code that blocks on rayon work from inside a [`Handle::scoped`] piece: while it
/// waits, the thread may steal an unrelated job (another trace's), and that job must not
/// find this trace's cancellation.
pub fn detached<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<Progress>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            WORKER.with(|w| *w.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(WORKER.with(|w| w.borrow_mut().take()));
    f()
}

/// This thread's progress, for a parallel loop to tick from rayon's workers. A handle taken
/// with nothing installed does nothing.
pub fn handle() -> Handle {
    Handle(current().map(|(p, _)| p))
}

/// A progress carried into a parallel loop. See [`handle`].
#[derive(Clone, Debug, Default)]
pub struct Handle(Option<Arc<Progress>>);

impl Handle {
    /// Run `f`, one piece of a parallel loop's work, on this thread with [`checkpoint`]
    /// reaching this handle's progress: a long piece stops in the middle when the trace is
    /// cancelled. The thread's previous scope is put back afterwards, also on unwind.
    ///
    /// Work this thread steals while `f` blocks on rayon would see the scope too; code that
    /// blocks that way inside `f` wraps the wait in [`detached`]. With nothing installed
    /// this is a plain call.
    pub fn scoped<R>(&self, f: impl FnOnce() -> R) -> R {
        let Some(p) = &self.0 else {
            return f();
        };
        struct Restore(Option<Arc<Progress>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                WORKER.with(|w| *w.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(WORKER.with(|w| w.borrow_mut().replace(Arc::clone(p))));
        f()
    }

    /// One more of the running step's units is done. Also a cancellation point.
    pub fn tick(&self) {
        if let Some(p) = &self.0 {
            p.check();
            p.done.fetch_add(1, Ordering::Relaxed);
            p.moved.fetch_add(1, Ordering::Release);
        }
    }

    /// A cancellation point, for a worker in the middle of a long piece of work.
    pub fn check(&self) {
        if let Some(p) = &self.0 {
            p.check();
        }
    }

    /// Whether this handle reports anywhere.
    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caught<R>(f: impl FnOnce() -> R) -> Result<R, Box<dyn std::any::Any + Send>> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
    }

    #[test]
    fn nothing_installed_is_a_no_op() {
        begin("palette");
        step("rounds", 1, 2);
        note(|| unreachable!("a note nobody reads is never built"));
        end("palette", 1.0);
        checkpoint();
        handle().tick();
        assert!(!active());
    }

    #[test]
    fn reports_arrive_in_order_and_the_step_only_when_it_moved() {
        let p = Arc::new(Progress::new());
        with(Arc::clone(&p), || {
            begin("merge_bands");
            note(|| "8 inks".into());
            step("rounds", 3, 0);
        });
        let r = p.take();
        assert!(matches!(
            r.entries[0],
            Entry::Begin {
                stage: "merge_bands",
                ..
            }
        ));
        assert!(
            matches!(&r.entries[1], Entry::Note { stage: "merge_bands", text, .. } if text == "8 inks")
        );
        assert_eq!(
            r.step,
            Some(Step {
                stage: "merge_bands",
                unit: "rounds",
                done: 3,
                total: 0
            })
        );
        assert!(p.take().is_empty(), "nothing new, nothing reported");
    }

    #[test]
    fn a_handle_ticks_from_other_threads() {
        let p = Arc::new(Progress::new());
        with(Arc::clone(&p), || {
            begin("fit_dp");
            step("rings", 0, 8);
            let h = handle();
            std::thread::scope(|s| {
                for _ in 0..8 {
                    let h = h.clone();
                    s.spawn(move || h.tick());
                }
            });
        });
        let step = p.take().step.expect("the ticks moved the step");
        assert_eq!((step.done, step.total, step.unit), (8, 8, "rings"));
    }

    #[test]
    fn a_cancelled_trace_unwinds_with_its_own_payload_and_restores_the_thread() {
        let p = Arc::new(Progress::new());
        let outer = Arc::new(Progress::new());
        let result = with(Arc::clone(&outer), || {
            let result = caught(|| {
                with(Arc::clone(&p), || {
                    begin("palette");
                    p.cancel();
                    step("rounds", 1, 0);
                    unreachable!("the step was a cancellation point");
                })
            });
            // The outer progress is back, and it is not the cancelled one.
            checkpoint();
            begin("carve");
            result
        });
        let payload = result.expect_err("cancelled");
        assert!(is_cancelled(payload.as_ref()));
        assert!(!outer.is_cancelled());
        assert!(matches!(
            outer.take().entries[..],
            [Entry::Begin { stage: "carve", .. }]
        ));
        assert!(!active(), "and nothing is left installed afterwards");
    }

    #[test]
    fn a_worker_stops_at_its_next_tick() {
        let p = Arc::new(Progress::new());
        let h = with(Arc::clone(&p), handle);
        h.tick();
        p.cancel();
        let err = caught(|| h.tick()).expect_err("cancelled");
        assert!(is_cancelled(err.as_ref()));
    }

    #[test]
    fn a_scoped_worker_stops_at_a_checkpoint_and_a_detached_one_does_not() {
        let p = Arc::new(Progress::new());
        let h = with(Arc::clone(&p), handle);
        p.cancel();
        std::thread::spawn(move || {
            // Outside the scope, a checkpoint on a worker knows nothing.
            checkpoint();
            let err = caught(|| h.scoped(checkpoint)).expect_err("cancelled");
            assert!(is_cancelled(err.as_ref()));
            // The scope is gone after the unwind.
            checkpoint();
            // Detached inside a scope: work stolen while waiting is not stopped.
            h.scoped(|| detached(checkpoint));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_real_panic_is_not_a_cancellation() {
        let err = caught(|| panic!("a bug")).expect_err("panicked");
        assert!(!is_cancelled(err.as_ref()));
    }

    #[test]
    fn wake_is_called_on_the_reporting_thread() {
        let seen = Rc::new(std::cell::Cell::new(0));
        let count = Rc::clone(&seen);
        let p = Arc::new(Progress::new());
        with_wake(
            Arc::clone(&p),
            move |_| count.set(count.get() + 1),
            || {
                begin("palette");
                step("rounds", 1, 0);
                end("palette", 2.0);
            },
        );
        assert_eq!(seen.get(), 3);
        assert_eq!(p.take().entries.len(), 2);
    }
}
