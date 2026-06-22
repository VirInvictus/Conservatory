//! Refresh orchestration: fetch → parse → upsert (Phase 6a-ii-b).
//!
//! Ties the [`Fetcher`](crate::fetcher) (6a-ii-a) to the [`parse`](crate::parse)
//! layer and the core single-writer worker. Three entry points:
//!
//! - [`add_show`]: subscribe to a new feed (`podcast add`). Unconditional
//!   fetch, create the show, upsert its episodes.
//! - [`refresh_show`]: re-poll one show with conditional GET, honouring its
//!   stored ETag / Last-Modified (a 304 just bumps `last_fetched`).
//! - [`refresh_all`]: poll every subscription concurrently under a
//!   [`Semaphore`], aggregating per-show outcomes.
//!
//! Triage (inbox policy, playback rows, queue insertion) is **not** here; that
//! is Phase 6b. Refresh only upserts episodes and stamps the conditional-GET
//! bookkeeping. User-configured show fields (priority, keep_count,
//! auto_download, auth, cover/accent) are preserved across a refresh: only the
//! descriptive metadata and the HTTP validators are rewritten.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use conservatory_core::db::{
    Episode, ReadPool, Show, WorkerHandle, get_show, list_episodes_for_show, list_shows,
};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::error::{FetchError, Result};
use crate::fetcher::Fetcher;
use crate::parse::{ParsedEpisode, ParsedFeed, parse_feed};
use crate::slug;

/// How many feeds to poll at once in [`refresh_all`]. Bounded so a large
/// subscription list does not open a connection per feed at once; the
/// per-request timeout (`http.rs`) keeps a dead host from holding a slot long.
const REFRESH_PARALLELISM: usize = 6;

/// What a single show's refresh did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshStatus {
    /// 2xx: the feed parsed and episodes were upserted. `new` counts episodes
    /// not previously seen (by `(show_id, guid)`); `total` is the feed's size.
    Updated { new: usize, total: usize },
    /// 304 Not Modified: nothing changed since the last poll.
    NotModified,
    /// Fetch or parse failed; the show keeps its prior state. The string is the
    /// error for display/logging.
    Failed(String),
}

/// One show's refresh result, carrying enough to report without a re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshOutcome {
    pub show_id: i64,
    pub show_title: String,
    pub status: RefreshStatus,
}

/// Subscribe to a new feed: fetch it, create (or resolve) the show, and upsert
/// its episodes. Returns the show id and the `(new, total)` episode counts.
///
/// Adding a feed that already exists is idempotent (the answer to the §6a-ii-b
/// open question): `get_or_create_show` returns the existing id and the call
/// simply refreshes that show's episodes rather than erroring.
pub async fn add_show(
    worker: &WorkerHandle,
    pool: &ReadPool,
    fetcher: &Fetcher,
    feed_url: &str,
) -> Result<(i64, usize, usize)> {
    let res = fetcher.fetch(feed_url, None, None).await?;
    let parsed = parse_feed(&res.body)?;

    let show_slug = slug::slugify(&parsed.channel.title);
    let skeleton = Show {
        id: 0,
        slug: show_slug.clone(),
        feed_url: feed_url.to_string(),
        title: parsed.channel.title.clone(),
        author: parsed.channel.author.clone(),
        description: parsed.channel.description.clone(),
        homepage_url: parsed.channel.homepage_url.clone(),
        cover_path: None,
        accent_rgb: None,
        apple_podcasts_id: None,
        last_fetched: Some(Utc::now()),
        last_modified: res.last_modified.clone(),
        etag: res.etag.clone(),
        fetch_interval: 3600,
        auth_user: None,
        auth_pass_ref: None,
        auto_download: true,
        keep_count: 0,
        priority: 0,
        folder_path: format!("{}/{}", slug::PODCASTS_DIR, show_slug),
    };

    let id = worker.get_or_create_show(skeleton).await?;

    // Resolve the canonical row (the just-created one, or a pre-existing
    // subscription with the same feed_url), then apply the feed to it so the
    // user's configured fields are preserved on a re-add.
    let show = {
        let conn = pool.open()?;
        get_show(&conn, id)?.ok_or_else(|| {
            FetchError::Parse(format!("show {id} vanished immediately after create"))
        })?
    };

    let (new, total) = apply_feed(worker, pool, show, parsed, res.etag, res.last_modified).await?;
    Ok((id, new, total))
}

/// Re-poll one subscription with conditional GET. Fetch and parse failures are
/// captured into a [`RefreshStatus::Failed`] outcome (so a batch refresh never
/// aborts on one bad feed); only a worker/DB error propagates as `Err`.
pub async fn refresh_show(
    worker: &WorkerHandle,
    pool: &ReadPool,
    fetcher: &Fetcher,
    show: Show,
) -> Result<RefreshOutcome> {
    let id = show.id;
    let title = show.title.clone();

    let res = match fetcher
        .fetch(
            &show.feed_url,
            show.etag.as_deref(),
            show.last_modified.as_deref(),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return Ok(outcome(id, title, RefreshStatus::Failed(e.to_string()))),
    };

    if res.status == 304 {
        // Not modified: bump only the poll timestamp, keep the validators.
        let mut bumped = show;
        bumped.last_fetched = Some(Utc::now());
        worker.update_show(bumped).await?;
        return Ok(outcome(id, title, RefreshStatus::NotModified));
    }

    let parsed = match parse_feed(&res.body) {
        Ok(p) => p,
        Err(e) => return Ok(outcome(id, title, RefreshStatus::Failed(e.to_string()))),
    };

    let (new, total) = apply_feed(worker, pool, show, parsed, res.etag, res.last_modified).await?;
    Ok(outcome(id, title, RefreshStatus::Updated { new, total }))
}

/// Poll every subscription concurrently (bounded by [`REFRESH_PARALLELISM`]).
/// Each show's outcome is collected; a task that hits a DB error or panics is
/// logged and dropped rather than failing the whole batch.
pub async fn refresh_all(
    worker: &WorkerHandle,
    pool: &ReadPool,
    fetcher: &Fetcher,
) -> Result<Vec<RefreshOutcome>> {
    let shows = {
        let conn = pool.open()?;
        list_shows(&conn)?
    };

    let sem = Arc::new(Semaphore::new(REFRESH_PARALLELISM));
    let mut set: JoinSet<Result<RefreshOutcome>> = JoinSet::new();
    for show in shows {
        let worker = worker.clone();
        let pool = pool.clone();
        let fetcher = fetcher.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.expect("refresh semaphore never closed");
            refresh_show(&worker, &pool, &fetcher, show).await
        });
    }

    let mut outcomes = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(outcome)) => outcomes.push(outcome),
            Ok(Err(e)) => tracing::error!(error = %e, "refresh: show task failed (db)"),
            Err(e) => tracing::error!(error = %e, "refresh: show task panicked"),
        }
    }
    Ok(outcomes)
}

/// Stamp a show's refreshed metadata + HTTP validators and upsert every parsed
/// episode. Returns `(new, total)`: episodes whose `(show_id, guid)` was not
/// already stored, and the feed's episode count.
async fn apply_feed(
    worker: &WorkerHandle,
    pool: &ReadPool,
    mut show: Show,
    parsed: ParsedFeed,
    etag: Option<String>,
    last_modified: Option<String>,
) -> Result<(usize, usize)> {
    // Refresh descriptive fields (feed is authoritative) but keep a prior value
    // when the feed omits one; rewrite the conditional-GET state outright.
    show.title = parsed.channel.title;
    show.author = parsed.channel.author.or(show.author);
    show.description = parsed.channel.description.or(show.description);
    show.homepage_url = parsed.channel.homepage_url.or(show.homepage_url);
    show.etag = etag;
    show.last_modified = last_modified;
    show.last_fetched = Some(Utc::now());

    let show_id = show.id;
    let show_slug = show.slug.clone();
    worker.update_show(show).await?;

    // Existing guids, read once, to count genuinely-new episodes.
    let existing: HashSet<String> = {
        let conn = pool.open()?;
        list_episodes_for_show(&conn, show_id)?
            .into_iter()
            .map(|e| e.guid)
            .collect()
    };

    let total = parsed.episodes.len();
    let mut new = 0;
    for pe in parsed.episodes {
        if !existing.contains(&pe.guid) {
            new += 1;
        }
        worker
            .upsert_episode(to_episode(show_id, &show_slug, pe))
            .await?;
    }
    Ok((new, total))
}

/// Build a core `Episode` from a parsed item. `audio_path` is `None` (download
/// is 6a-iii); `folder_path` is the managed download dir (spec §5.3), computed
/// now so the row is download-ready.
fn to_episode(show_id: i64, show_slug: &str, pe: ParsedEpisode) -> Episode {
    let folder_path = slug::episode_dir(show_slug, pe.pub_date, &pe.title);
    Episode {
        id: 0,
        show_id,
        guid: pe.guid,
        title: pe.title,
        description: pe.description,
        pub_date: pe.pub_date,
        duration: pe.duration,
        file_size: pe.file_size,
        audio_url: pe.audio_url,
        audio_path: None,
        folder_path,
        mime_type: pe.mime_type,
        season: pe.season,
        episode_number: pe.episode_number,
        episode_type: pe.episode_type,
    }
}

fn outcome(show_id: i64, show_title: String, status: RefreshStatus) -> RefreshOutcome {
    RefreshOutcome {
        show_id,
        show_title,
        status,
    }
}
