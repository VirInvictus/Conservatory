//! MPRIS2 + the suspend inhibitor (spec §6.5, Phase 4c-i).
//!
//! Serves `org.mpris.MediaPlayer2` and `…Player` on the session bus so GNOME's
//! media overlay, lock screen, and the keyboard media keys drive the engine;
//! and holds a logind "sleep" inhibitor on the system bus while playing. Both
//! live in core (the player and its surfaces are core, spec §16.13) and are
//! spawned on the GUI's tokio runtime via [`run`].
//!
//! The state→D-Bus mapping is factored into pure helpers (unit-tested); the
//! `run` loop polls the player snapshot, emits `PropertiesChanged` on change,
//! and acquires/releases the inhibitor. There is no D-Bus precedent in the
//! sibling projects; this is built on `zbus 5`.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{connection, interface, proxy};

use crate::db::{ReadPool, track_metadata};
use crate::errors::Result;
use crate::player::{PlayerHandle, PlayerSnapshot};

const BUS_NAME: &str = "org.mpris.MediaPlayer2.conservatory";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

// --- Pure state→MPRIS mapping (unit-tested, no bus) ---

/// The MPRIS `PlaybackStatus` for a snapshot.
pub(crate) fn playback_status(snap: &PlayerSnapshot) -> &'static str {
    if snap.current_index.is_none() || snap.ended {
        "Stopped"
    } else if snap.paused {
        "Paused"
    } else {
        "Playing"
    }
}

pub(crate) fn can_go_next(snap: &PlayerSnapshot) -> bool {
    matches!(snap.current_index, Some(i) if i + 1 < snap.queue_len)
}

pub(crate) fn can_go_previous(snap: &PlayerSnapshot) -> bool {
    matches!(snap.current_index, Some(i) if i > 0)
}

/// Whether playback warrants a suspend inhibitor (something loaded, playing).
pub(crate) fn wants_inhibit(snap: &PlayerSnapshot) -> bool {
    snap.current_index.is_some() && !snap.paused && !snap.ended
}

pub(crate) fn volume_to_mpris(volume: i64) -> f64 {
    (volume as f64 / 100.0).clamp(0.0, 1.0)
}

pub(crate) fn volume_from_mpris(volume: f64) -> i64 {
    (volume * 100.0).round().clamp(0.0, 100.0) as i64
}

pub(crate) fn position_us(secs: f64) -> i64 {
    (secs.max(0.0) * 1_000_000.0) as i64
}

/// Build the MPRIS `Metadata` (`a{sv}`) from the current track id + resolved
/// fields. `root` resolves the album cover path into the `mpris:artUrl` file URL.
/// Empty (just a NoTrack id) when nothing is playing.
fn build_metadata(
    track_id: Option<i64>,
    meta: Option<&crate::db::NowPlaying>,
    root: &std::path::Path,
) -> Metadata {
    let mut m: Metadata = HashMap::new();
    let path = match track_id {
        Some(id) => format!("/org/conservatory/track/{id}"),
        None => "/org/mpris/MediaPlayer2/TrackList/NoTrack".to_string(),
    };
    if let Ok(op) = ObjectPath::try_from(path)
        && let Ok(v) = Value::from(op).try_to_owned()
    {
        m.insert("mpris:trackid".to_string(), v);
    }
    if let Some(np) = meta {
        if let Ok(v) = Value::from(np.title.clone()).try_to_owned() {
            m.insert("xesam:title".to_string(), v);
        }
        if let Some(artist) = &np.artist
            && let Ok(v) = Value::from(vec![artist.clone()]).try_to_owned()
        {
            m.insert("xesam:artist".to_string(), v);
        }
        if let Some(album) = &np.album
            && let Ok(v) = Value::from(album.clone()).try_to_owned()
        {
            m.insert("xesam:album".to_string(), v);
        }
        if let Some(len) = np.length
            && let Ok(v) = Value::from(position_us(len)).try_to_owned()
        {
            m.insert("mpris:length".to_string(), v);
        }
        if let Some(cover) = &np.album_cover_path {
            let abs = root.join(cover);
            if let Ok(v) = Value::from(format!("file://{}", abs.display())).try_to_owned() {
                m.insert("mpris:artUrl".to_string(), v);
            }
        }
    }
    m
}

type Metadata = HashMap<String, OwnedValue>;

// --- org.mpris.MediaPlayer2 (root) ---

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    async fn raise(&self) {}
    async fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Conservatory"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

// --- org.mpris.MediaPlayer2.Player ---

/// The transport interface. Property getters return cached state the `run` loop
/// refreshes (so getters touch no DB); methods call straight into the engine.
struct Player {
    handle: PlayerHandle,
    status: String,
    metadata: Metadata,
    volume: f64,
    can_next: bool,
    can_prev: bool,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    async fn play(&self) {
        self.handle.play();
    }
    async fn pause(&self) {
        self.handle.pause();
    }
    async fn play_pause(&self) {
        self.handle.toggle_pause();
    }
    async fn stop(&self) {
        self.handle.stop();
    }
    async fn next(&self) {
        self.handle.next();
    }
    async fn previous(&self) {
        self.handle.previous();
    }
    /// `Seek(offset)` is relative, in microseconds.
    async fn seek(&self, offset: i64) {
        let target = self.handle.snapshot().position + offset as f64 / 1_000_000.0;
        self.handle.seek(target.max(0.0));
    }
    /// `SetPosition(trackid, position)` is absolute, in microseconds.
    async fn set_position(&self, _track: ObjectPath<'_>, position: i64) {
        self.handle.seek((position as f64 / 1_000_000.0).max(0.0));
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.status.clone()
    }
    #[zbus(property)]
    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        position_us(self.handle.snapshot().position)
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.volume
    }
    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        self.handle.set_volume(volume_from_mpris(volume));
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.can_next
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.can_prev
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

// --- logind suspend inhibitor (system bus) ---

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    fn inhibit(
        &self,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedFd>;
}

/// Serve MPRIS and drive the inhibitor until the runtime is torn down. Spawned
/// by the GUI: `rt.spawn(mpris::run(player, pool, root))`. `root` resolves the
/// album cover path into `mpris:artUrl`.
pub async fn run(handle: PlayerHandle, pool: ReadPool, root: std::path::PathBuf) -> Result<()> {
    let snap = handle.snapshot();
    let mut last_track = snap.track_id;
    let metadata = build_metadata(
        snap.track_id,
        current_meta(&pool, snap.track_id).as_ref(),
        &root,
    );

    let player = Player {
        handle: handle.clone(),
        status: playback_status(&snap).to_string(),
        metadata,
        volume: volume_to_mpris(snap.volume),
        can_next: can_go_next(&snap),
        can_prev: can_go_previous(&snap),
    };

    let conn = connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Root)?
        .serve_at(OBJECT_PATH, player)?
        .build()
        .await?;

    let iface_ref = conn
        .object_server()
        .interface::<_, Player>(OBJECT_PATH)
        .await?;

    // The inhibitor is best-effort: a missing system bus / logind must not kill
    // the MPRIS surface.
    let login1 = match zbus::Connection::system().await {
        Ok(c) => Login1ManagerProxy::new(&c).await.ok(),
        Err(e) => {
            tracing::debug!(error = %e, "no system bus; suspend inhibitor disabled");
            None
        }
    };
    let mut inhibit_fd: Option<zbus::zvariant::OwnedFd> = None;

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(300));
    loop {
        tick.tick().await;
        let snap = handle.snapshot();

        let status = playback_status(&snap).to_string();
        let volume = volume_to_mpris(snap.volume);
        let can_next = can_go_next(&snap);
        let can_prev = can_go_previous(&snap);

        let mut iface = iface_ref.get_mut().await;
        if iface.status != status {
            iface.status = status;
            let _ = iface
                .playback_status_changed(iface_ref.signal_emitter())
                .await;
        }
        if (iface.volume - volume).abs() > f64::EPSILON {
            iface.volume = volume;
            let _ = iface.volume_changed(iface_ref.signal_emitter()).await;
        }
        if iface.can_next != can_next {
            iface.can_next = can_next;
            let _ = iface.can_go_next_changed(iface_ref.signal_emitter()).await;
        }
        if iface.can_prev != can_prev {
            iface.can_prev = can_prev;
            let _ = iface
                .can_go_previous_changed(iface_ref.signal_emitter())
                .await;
        }
        if snap.track_id != last_track {
            last_track = snap.track_id;
            iface.metadata = build_metadata(
                snap.track_id,
                current_meta(&pool, snap.track_id).as_ref(),
                &root,
            );
            let _ = iface.metadata_changed(iface_ref.signal_emitter()).await;
        }
        drop(iface);

        // Acquire/release the suspend inhibitor on the playing↔idle transition.
        if let Some(login1) = &login1 {
            let want = wants_inhibit(&snap);
            if want && inhibit_fd.is_none() {
                match login1
                    .inhibit("sleep", "Conservatory", "Playing audio", "block")
                    .await
                {
                    Ok(fd) => inhibit_fd = Some(fd),
                    Err(e) => tracing::debug!(error = %e, "inhibit failed"),
                }
            } else if !want && inhibit_fd.is_some() {
                inhibit_fd = None; // dropping the fd releases the inhibitor
            }
        }
    }
}

/// Resolve the current track id's metadata, swallowing read errors (the MPRIS
/// surface should degrade, not crash).
fn current_meta(pool: &ReadPool, track_id: Option<i64>) -> Option<crate::db::NowPlaying> {
    let id = track_id?;
    pool.open()
        .ok()
        .and_then(|conn| track_metadata(&conn, id).ok().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(current: Option<usize>, queue_len: usize, paused: bool, ended: bool) -> PlayerSnapshot {
        PlayerSnapshot {
            current_index: current,
            track_id: current.map(|i| i as i64 + 1),
            paused,
            queue_len,
            ended,
            ..Default::default()
        }
    }

    #[test]
    fn status_maps() {
        assert_eq!(playback_status(&snap(None, 0, false, false)), "Stopped");
        assert_eq!(playback_status(&snap(Some(0), 3, false, true)), "Stopped");
        assert_eq!(playback_status(&snap(Some(0), 3, true, false)), "Paused");
        assert_eq!(playback_status(&snap(Some(0), 3, false, false)), "Playing");
    }

    #[test]
    fn can_go_at_the_ends() {
        assert!(!can_go_previous(&snap(Some(0), 3, false, false)));
        assert!(can_go_next(&snap(Some(0), 3, false, false)));
        assert!(can_go_previous(&snap(Some(2), 3, false, false)));
        assert!(!can_go_next(&snap(Some(2), 3, false, false)));
        assert!(!can_go_next(&snap(None, 0, false, false)));
    }

    #[test]
    fn inhibit_only_while_playing() {
        assert!(wants_inhibit(&snap(Some(0), 3, false, false)));
        assert!(!wants_inhibit(&snap(Some(0), 3, true, false)));
        assert!(!wants_inhibit(&snap(Some(0), 3, false, true)));
        assert!(!wants_inhibit(&snap(None, 0, false, false)));
    }

    #[test]
    fn volume_and_position_convert() {
        assert_eq!(volume_to_mpris(100), 1.0);
        assert_eq!(volume_to_mpris(0), 0.0);
        assert_eq!(volume_from_mpris(0.5), 50);
        assert_eq!(volume_from_mpris(1.5), 100); // clamped
        assert_eq!(position_us(1.5), 1_500_000);
        assert_eq!(position_us(-3.0), 0);
    }

    #[test]
    fn metadata_has_core_fields() {
        let np = crate::db::NowPlaying {
            title: "Roygbiv".into(),
            artist: Some("Boards of Canada".into()),
            album: Some("Music Has the Right".into()),
            length: Some(2.0),
            album_cover_path: Some("Electronic/Boards of Canada/Music (1998)/cover.jpg".into()),
        };
        let m = build_metadata(Some(7), Some(&np), std::path::Path::new("/lib"));
        assert!(m.contains_key("mpris:trackid"));
        assert!(m.contains_key("xesam:title"));
        assert!(m.contains_key("xesam:artist"));
        assert!(m.contains_key("mpris:length"));
        assert!(m.contains_key("mpris:artUrl"), "cover path yields artUrl");
    }

    #[test]
    fn metadata_without_cover_has_no_arturl() {
        let np = crate::db::NowPlaying {
            title: "X".into(),
            artist: None,
            album: None,
            length: None,
            album_cover_path: None,
        };
        let m = build_metadata(Some(1), Some(&np), std::path::Path::new("/lib"));
        assert!(!m.contains_key("mpris:artUrl"));
    }
}
