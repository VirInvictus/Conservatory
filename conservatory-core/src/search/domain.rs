use vir_search::ast::{FieldType, ParseField, ParseSort, ParseState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Genre,      // raw multi-value tags (the §5.2 facet side)
    ShelfGenre, // single-valued filed-under (the §5.2 filesystem side)
    Year,
    Added,
    Rating,
    Bitrate,
    Duration,
    Format,
    // Audiobook text fields (spec §3.8); matched in-memory only, never pushed to
    // the music `tracks` SQL (the `books` shelf is evaluated in memory).
    Author,
    Narrator,
    Series,
}
impl Field {
    /// Resolve a (lowercased) field token; `None` means "not a known field".
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "artist" => Self::Artist,
            "albumartist" | "album_artist" => Self::AlbumArtist,
            "album" => Self::Album,
            "title" => Self::Title,
            "genre" => Self::Genre,
            "shelfgenre" | "shelf_genre" => Self::ShelfGenre,
            "year" => Self::Year,
            "added" => Self::Added,
            "rating" => Self::Rating,
            "bitrate" => Self::Bitrate,
            "duration" => Self::Duration,
            "format" => Self::Format,
            "author" => Self::Author,
            "narrator" => Self::Narrator,
            "series" => Self::Series,
            _ => return None,
        })
    }

    /// The canonical token (what `Display` emits).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::AlbumArtist => "albumartist",
            Self::Album => "album",
            Self::Title => "title",
            Self::Genre => "genre",
            Self::ShelfGenre => "shelfgenre",
            Self::Year => "year",
            Self::Added => "added",
            Self::Rating => "rating",
            Self::Bitrate => "bitrate",
            Self::Duration => "duration",
            Self::Format => "format",
            Self::Author => "author",
            Self::Narrator => "narrator",
            Self::Series => "series",
        }
    }

    /// Whether the field is numeric (drives compare/range vs text matching).
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::Year | Self::Rating | Self::Bitrate | Self::Duration
        )
    }

    pub fn is_date(self) -> bool {
        matches!(self, Self::Added)
    }
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
impl ParseField for Field {
    fn parse(name: &str) -> Option<Self> {
        Self::parse(name)
    }
    fn field_type(&self) -> FieldType {
        if self.is_date() {
            FieldType::Date
        } else if self.is_numeric() {
            if matches!(self, Self::Duration) {
                FieldType::Real
            } else {
                FieldType::Int
            }
        } else {
            FieldType::String
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Played,
    Starred,
    Queued,
    /// An audiobook the listener has finished (spec §3.8). Negate with
    /// `NOT is:finished`, the same shape as every other state.
    Finished,
}
impl State {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "played" => Self::Played,
            "starred" => Self::Starred,
            "queued" => Self::Queued,
            "finished" => Self::Finished,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Played => "played",
            Self::Starred => "starred",
            Self::Queued => "queued",
            Self::Finished => "finished",
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
impl ParseState for State {
    fn parse(name: &str) -> Option<Self> {
        Self::parse(name)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Title,
    Artist,
    Album,
    Year,
    Added,
    Rating,
    Duration,
}
impl SortKey {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "title" => Self::Title,
            "artist" => Self::Artist,
            "album" => Self::Album,
            "year" => Self::Year,
            "added" => Self::Added,
            "rating" => Self::Rating,
            "duration" => Self::Duration,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Year => "year",
            Self::Added => "added",
            Self::Rating => "rating",
            Self::Duration => "duration",
        }
    }
}

impl std::fmt::Display for SortKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
impl ParseSort for SortKey {
    fn parse(name: &str) -> Option<Self> {
        Self::parse(name)
    }
}
