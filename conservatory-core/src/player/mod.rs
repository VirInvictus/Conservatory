//! Playback engine (spec §6, docs/libmpv-profiles.md).
//!
//! A single libmpv instance plays a queue of items, each resolved to a playback
//! profile applied before it plays. Phase 4a lands the music half: the libmpv
//! host ([`host`]), the music profile resolution ([`profile`]), and the
//! position/play-count persistence logic ([`state`]). The unified queue, the
//! Now-bar, MPRIS, and the podcast/audiobook spoken-word profile follow at
//! Phases 4b, 4c, and 6c.
//!
//! The split is deliberate (the CLAUDE.md rule, spec §16.13): `profile` and
//! `state` are pure and unit-tested headless; `host` is the thin libmpv glue,
//! kept in core (not the GTK binary) so the whole engine stays CLI-driveable.

pub mod book;
pub mod chain;
pub mod chapters;
pub mod dsp;
pub mod engine;
pub mod handle;
pub mod host;
pub mod item;
pub mod profile;
pub mod session;
pub mod sleep;
pub mod spoken;
pub mod state;

pub use book::{BookPlan, BookSegment, build_book_item, locate, plan_book};
pub use chain::{build_af_chain, eq_band_command, eq_stage};
pub use chapters::{ChapterMark, current_chapter_at, neighbour_chapter};
pub use dsp::{comp_stage, leveler_stage, limiter_stage};
pub use engine::{spawn, spawn_null, spawn_with};
pub use handle::{PlayerCommand, PlayerHandle, PlayerSnapshot};
pub use host::{AudioDevice, HostEvent, MpvHost};
pub use item::PlayableItem;
pub use profile::{
    MusicProfile, PlaybackConfig, ReplayGain, resolve_book_profile, resolve_episode_profile,
    resolve_music_profile,
};
pub use session::{SessionAccumulator, SessionOwner};
pub use sleep::{SleepClock, SleepMode, SleepStatus};
pub use spoken::{smart_speed_stage, voice_boost_stages};
pub use state::{EndReason, INSURANCE_INTERVAL_MS, StateDebounce, StateEvent};
