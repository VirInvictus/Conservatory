//! Transport play-order modes (Phase 17): repeat, and the shuffle flag's home.
//!
//! Pure and db-free: [`AudioState`](crate::db::AudioState) stores these as TEXT /
//! bool to stay enum-free (the `replaygain_mode` idiom), and this is the one place
//! the string becomes a [`Repeat`]. The engine reads the resolved enum; the GUI
//! cycles it and persists the string back.

/// The repeat mode (spec §6, Phase 17a): what the engine does at the end of the
/// queue / the current item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    /// Play to the end of the queue and stop (the pre-17 behaviour).
    #[default]
    Off,
    /// At the end of the queue, wrap to the first item and keep playing (with
    /// shuffle on, the lap is reshuffled first, Phase 17b).
    All,
    /// Replay the current item forever; the queue never advances.
    One,
}

impl Repeat {
    /// The stored / displayed token (the `ReplayGain::as_str` idiom).
    pub fn as_str(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }

    /// Parse the stored token, degrading an unrecognized value to `Off` (the
    /// forgiving read `get_audio_state` uses for `replaygain_mode`).
    pub fn from_stored(s: &str) -> Self {
        match s {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }

    /// The next mode in the cycle the Now-bar button walks: Off → All → One → Off.
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_round_trips() {
        for m in [Repeat::Off, Repeat::All, Repeat::One] {
            assert_eq!(Repeat::from_stored(m.as_str()), m);
        }
    }

    #[test]
    fn unknown_stored_degrades_to_off() {
        assert_eq!(Repeat::from_stored("nonsense"), Repeat::Off);
        assert_eq!(Repeat::from_stored(""), Repeat::Off);
    }

    #[test]
    fn cycle_walks_off_all_one() {
        assert_eq!(Repeat::Off.next(), Repeat::All);
        assert_eq!(Repeat::All.next(), Repeat::One);
        assert_eq!(Repeat::One.next(), Repeat::Off);
    }

    #[test]
    fn default_is_off() {
        assert_eq!(Repeat::default(), Repeat::Off);
    }
}
