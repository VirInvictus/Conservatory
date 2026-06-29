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
}

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
}
