//! A debounce/coalesce queue, ported in shape from Viaduct's `CoalescingQueue`
//! (spec §2.1). Bursts of `add()` within `interval` collapse into a single
//! `perform` call carrying the deduped batch; `max_interval` bounds the wait so a
//! continuous stream still flushes. Used to debounce facet-pane selection changes
//! into one cascade recompute (the deadbeef-cui invariant), never per-row.
//!
//! Main-thread only (`glib::timeout_add_local_once`); not `Send`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk4 as gtk;

struct Inner<T, F> {
    interval: Duration,
    max_interval: Duration,
    last_flush: Instant,
    timer: Option<glib::SourceId>,
    pending: Vec<T>,
    /// `None` only while `perform` is executing (taken out so the borrow is
    /// released and the closure may re-enter `add`/`flush`).
    perform: Option<F>,
}

/// A coalescing queue. Cheap to clone (shares one `Rc`).
pub struct CoalescingQueue<T, F>
where
    T: PartialEq + Clone + 'static,
    F: FnMut(Vec<T>) + 'static,
{
    inner: Rc<RefCell<Inner<T, F>>>,
}

// Manual `Clone` so it does not require `F: Clone` (the closure is shared via Rc).
impl<T, F> Clone for CoalescingQueue<T, F>
where
    T: PartialEq + Clone + 'static,
    F: FnMut(Vec<T>) + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T, F> CoalescingQueue<T, F>
where
    T: PartialEq + Clone + 'static,
    F: FnMut(Vec<T>) + 'static,
{
    pub fn new(interval: Duration, max_interval: Duration, perform: F) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                interval,
                max_interval,
                last_flush: Instant::now(),
                timer: None,
                pending: Vec::new(),
                perform: Some(perform),
            })),
        }
    }

    /// Queue a call. Deduped against pending items; (re)arms the flush timer, or
    /// flushes immediately if `max_interval` has elapsed since the last flush.
    pub fn add(&self, call: T) {
        let mut inner = self.inner.borrow_mut();
        if !inner.pending.contains(&call) {
            inner.pending.push(call);
        }
        if inner.last_flush.elapsed() >= inner.max_interval {
            drop(inner);
            self.flush();
            return;
        }
        if let Some(id) = inner.timer.take() {
            id.remove();
        }
        let interval = inner.interval;
        let this = self.clone();
        inner.timer = Some(glib::timeout_add_local_once(interval, move || this.flush()));
    }

    /// Flush now: deliver the coalesced batch to `perform`.
    pub fn flush(&self) {
        let (batch, perform) = {
            let mut inner = self.inner.borrow_mut();
            if let Some(id) = inner.timer.take() {
                id.remove();
            }
            inner.last_flush = Instant::now();
            (std::mem::take(&mut inner.pending), inner.perform.take())
        };
        if batch.is_empty() {
            if let Some(f) = perform {
                self.inner.borrow_mut().perform = Some(f);
            }
            return;
        }
        let Some(mut perform) = perform else {
            // Re-entrant flush from inside `perform`: the closure is already
            // running. Put the batch back (ahead of anything queued since) and
            // let the tail below re-arm the timer when the outer call returns.
            let mut inner = self.inner.borrow_mut();
            let queued_since = std::mem::replace(&mut inner.pending, batch);
            for call in queued_since {
                if !inner.pending.contains(&call) {
                    inner.pending.push(call);
                }
            }
            return;
        };
        // The borrow is released here, so `perform` may re-enter `add`/`flush`.
        perform(batch);
        let mut inner = self.inner.borrow_mut();
        inner.perform = Some(perform);
        // Anything queued re-entrantly during `perform` lost its timer if it
        // took the immediate-flush path; make sure a pending batch always has
        // a live timer so it cannot strand.
        if !inner.pending.is_empty() && inner.timer.is_none() {
            let interval = inner.interval;
            let this = self.clone();
            inner.timer = Some(glib::timeout_add_local_once(interval, move || this.flush()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// Run `f` holding the global default main context. `timeout_add_local_once`
    /// (in both the queue and each test's quit timer) attaches to the global
    /// default context, so the tests must share it; the mutex serializes them
    /// (parallel test threads would otherwise fail the `acquire`).
    fn in_default_context(f: impl FnOnce(&glib::MainContext)) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = glib::MainContext::default();
        let _guard = ctx.acquire().unwrap();
        f(&ctx);
    }

    #[test]
    fn burst_coalesces_into_one_flush() {
        in_default_context(|ctx| {
            let flushes: Rc<RefCell<Vec<Vec<u32>>>> = Rc::new(RefCell::new(Vec::new()));
            let sink = flushes.clone();
            let q = CoalescingQueue::new(
                Duration::from_millis(30),
                Duration::from_secs(10),
                move |batch| sink.borrow_mut().push(batch),
            );

            // A burst within the interval, with a duplicate.
            q.add(2);
            q.add(1);
            q.add(2);

            // Run the context until the timer fires.
            let loop_ = glib::MainLoop::new(Some(ctx), false);
            let l = loop_.clone();
            glib::timeout_add_local_once(Duration::from_millis(120), move || l.quit());
            loop_.run();

            let got = flushes.borrow();
            assert_eq!(got.len(), 1, "burst should flush once");
            assert_eq!(got[0], vec![2, 1], "deduped, insertion order preserved");
        });
    }

    #[test]
    fn perform_may_reenter_add_without_panicking() {
        // The module contract: `perform` runs outside the borrow, so a closure
        // may queue follow-up work on the same queue. This used to panic with
        // BorrowMutError; the re-queued item must flush in a later batch.
        in_default_context(|ctx| {
            // Boxed closure type so the queue can be captured by its own
            // `perform` without a cyclic (infinite) type.
            type ReentrantQueue = CoalescingQueue<u32, Box<dyn FnMut(Vec<u32>)>>;

            let flushes: Rc<RefCell<Vec<Vec<u32>>>> = Rc::new(RefCell::new(Vec::new()));
            let q: Rc<RefCell<Option<ReentrantQueue>>> = Rc::new(RefCell::new(None));
            let sink = flushes.clone();
            let q_for_perform = q.clone();
            let queue: ReentrantQueue = CoalescingQueue::new(
                Duration::from_millis(20),
                Duration::from_secs(10),
                Box::new(move |batch: Vec<u32>| {
                    if batch == [1] {
                        // Re-enter `add` from inside `perform`.
                        q_for_perform.borrow().as_ref().unwrap().add(2);
                    }
                    sink.borrow_mut().push(batch);
                }),
            );
            *q.borrow_mut() = Some(queue.clone());

            queue.add(1);

            let loop_ = glib::MainLoop::new(Some(ctx), false);
            let l = loop_.clone();
            glib::timeout_add_local_once(Duration::from_millis(150), move || l.quit());
            loop_.run();

            let got = flushes.borrow();
            assert_eq!(*got, vec![vec![1], vec![2]], "re-entrant add flushes later");

            // Drop the queue's self-reference so the Rc cycle doesn't outlive
            // the context (hygiene only; the test process would clean it up).
            q.borrow_mut().take();
        });
    }
}
