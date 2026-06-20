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
    perform: F,
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
                perform,
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
        let (batch, ()) = {
            let mut inner = self.inner.borrow_mut();
            if let Some(id) = inner.timer.take() {
                id.remove();
            }
            inner.last_flush = Instant::now();
            (std::mem::take(&mut inner.pending), ())
        };
        if batch.is_empty() {
            return;
        }
        // Call `perform` outside the borrow so it may re-enter `add`.
        let mut inner = self.inner.borrow_mut();
        (inner.perform)(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn burst_coalesces_into_one_flush() {
        // Use the thread-default context: `timeout_add_local_once` (in both the
        // queue and the quit timer below) attaches to it, so the loop sees them.
        let ctx = glib::MainContext::default();
        let _guard = ctx.acquire().unwrap();

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
        let loop_ = glib::MainLoop::new(Some(&ctx), false);
        let l = loop_.clone();
        glib::timeout_add_local_once(Duration::from_millis(120), move || l.quit());
        loop_.run();

        let got = flushes.borrow();
        assert_eq!(got.len(), 1, "burst should flush once");
        assert_eq!(got[0], vec![2, 1], "deduped, insertion order preserved");
    }
}
