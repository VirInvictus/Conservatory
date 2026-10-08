//! A per-session cache of downscaled cover textures (Phase 12b), so the browse
//! cover column does not re-decode a full-resolution `cover.jpg` on every
//! scroll-bind. Keyed by absolute path; cheap to clone (an `Rc`), so one cache is
//! shared across the column's factory closures.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gdk, gdk_pixbuf};
use gtk4 as gtk;

#[derive(Clone, Default)]
pub struct CoverCache {
    inner: Rc<RefCell<HashMap<PathBuf, gdk::Texture>>>,
    /// Cover decodes queued but not done, in submit order: the budgeted idle
    /// drain (see [`request`](Self::request)) pops from the front.
    pending: Rc<RefCell<std::collections::VecDeque<QueuedDecode>>>,
    /// Whether an idle drain is armed (so only the first queue-submit arms one).
    draining: Rc<std::cell::Cell<bool>>,
}

/// The repaint hook a requesting cell hands over: re-validates its own widget
/// on delivery, so a recycled cell never paints another row's cover.
pub type CoverRepaint = Rc<dyn Fn(&Path, Option<gdk::Texture>)>;

/// One queued decode: the cover path, the render size, and the repaint hook.
type QueuedDecode = (PathBuf, i32, CoverRepaint);

impl CoverCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cover texture downscaled to fit `size`x`size` px, or `None` when the
    /// file is absent or cannot be decoded. Decoded once per path, then reused
    /// (decode misses are cheap, so they are not cached).
    pub fn texture(&self, path: &Path, size: i32) -> Option<gdk::Texture> {
        if let Some(tex) = self.inner.borrow().get(path) {
            return Some(tex.clone());
        }
        let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(path, size, size, true).ok()?;
        let tex = gdk::Texture::for_pixbuf(&pixbuf);
        self.inner
            .borrow_mut()
            .insert(path.to_path_buf(), tex.clone());
        Some(tex)
    }

    /// Queue a cover decode for a cache miss, to run on the main loop in
    /// time-budgeted slices: the synchronous decode inside a bind was the
    /// browse jank on a big library (a cold facet painted ~30 covers at
    /// 10-30 ms each, inline). Returns `true` when the decode was queued (the
    /// caller shows the placeholder; the repaint hook it passed re-validates
    /// its own widget before painting), `false` when the texture is already
    /// cached.
    pub fn request(
        &self,
        path: PathBuf,
        size: i32,
        repaint: CoverRepaint,
    ) -> bool {
        if self.inner.borrow().contains_key(&path) {
            return false;
        }
        self.pending.borrow_mut().push_back((path, size, repaint));
        if self.draining.replace(true) {
            return true;
        }
        let cache = self.clone();
        gtk::glib::idle_add_local(move || cache.drain_slice());
        true
    }

    /// One budgeted drain slice: decode queued covers until the time budget is
    /// spent or the queue empties, repainting each requester. Runs at default
    /// idle priority, so input and layout always outrank it.
    fn drain_slice(&self) -> gtk::glib::ControlFlow {
        let budget = std::time::Instant::now() + std::time::Duration::from_millis(8);
        loop {
            let next = self.pending.borrow_mut().pop_front();
            let Some((path, size, repaint)) = next else {
                self.draining.set(false);
                return gtk::glib::ControlFlow::Break;
            };
            // A bind may have delivered the texture while this entry waited.
            let tex = self.texture(&path, size).or_else(|| {
                gdk_pixbuf::Pixbuf::from_file_at_scale(&path, size, size, true)
                    .ok()
                    .map(|pb| gdk::Texture::for_pixbuf(&pb))
            });
            if let Some(tex) = &tex {
                self.inner.borrow_mut().insert(path.clone(), tex.clone());
            }
            repaint(&path, tex);
            if std::time::Instant::now() >= budget {
                return gtk::glib::ControlFlow::Continue;
            }
        }
    }
}
