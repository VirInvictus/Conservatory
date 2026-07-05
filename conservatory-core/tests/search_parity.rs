//! Phase 3a: the SQL fast path and the in-memory evaluator must return the same
//! track set for any translatable expression (the all-or-nothing guarantee). We
//! run a set of fielded expressions over a fixture library through both paths and
//! assert the id sets match. Bare text is excluded (it uses FTS in SQL vs
//! substring in eval by design); regex/fuzzy are excluded (they don't translate).

use std::collections::HashSet;

use chrono::NaiveDate;
use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    ReadPool, SearchRow, SqlParam, search_rows, search_track_ids, spawn_worker,
};
use conservatory_search::{SearchItem, SqlValue, evaluate, parse, try_translate};
use tempfile::tempdir;

fn to_item(r: &SearchRow) -> SearchItem {
    SearchItem {
        title: r.title.clone(),
        artist: r.artist.clone(),
        album_artist: r.album_artist.clone(),
        album: r.album.clone(),
        shelf_genre: r.shelf_genre.clone(),
        genres: r.genres.clone(),
        year: r.year,
        added: r.added,
        rating: r.rating,
        bitrate: r.bitrate,
        duration: r.duration,
        format: r.format.clone(),
        played: r.played,
        starred: r.starred,
        queued: r.queued,
        // Music rows carry no audiobook projection (the shelf is matched in memory).
        ..SearchItem::default()
    }
}

fn to_param(v: &SqlValue) -> SqlParam {
    match v {
        SqlValue::Text(s) => SqlParam::Text(s.clone()),
        SqlValue::Int(n) => SqlParam::Int(*n),
        SqlValue::Real(x) => SqlParam::Real(*x),
    }
}

#[tokio::test]
async fn sql_and_eval_agree_on_the_translatable_subset() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Medium)
        .await
        .unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();
    let rows = search_rows(&conn).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();

    let expressions = [
        "genre:Electronic",
        "genre:electronic", // case-insensitive
        "shelfgenre:Jazz",
        "format:flac",
        "year:>=1991",
        "year:1990..1991",
        "rating:>=0", // all
        "rating:>0",  // none (fixture rating is 0)
        "bitrate:>=1000",
        "artist:\"Artist 0001\"",
        "album:\"Album 1-0\"",
        "genre:Electronic AND year:1990",
        "genre:Electronic OR genre:Jazz",
        "NOT genre:Jazz",
        "format:true",
        "rating:false",
    ];

    for expr in expressions {
        let parsed = parse(expr);
        let clause = try_translate(&parsed.expr, today)
            .unwrap_or_else(|| panic!("{expr:?} should translate"));

        let params: Vec<SqlParam> = clause.params.iter().map(to_param).collect();
        let sql_ids: HashSet<i64> = search_track_ids(&conn, &clause.sql, &params)
            .unwrap()
            .into_iter()
            .collect();

        let eval_ids: HashSet<i64> = rows
            .iter()
            .filter(|r| evaluate(&parsed.expr, &to_item(r), today))
            .map(|r| r.track_id)
            .collect();

        assert_eq!(sql_ids, eval_ids, "path divergence for {expr:?}");
    }

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn regex_does_not_translate_so_eval_runs() {
    let today = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();
    assert!(try_translate(&parse("title:~^track").expr, today).is_none());
    assert!(try_translate(&parse("artist:?artst").expr, today).is_none());
}

/// Phase 18a: bare-text search folds accents on the SQL fast path. A track whose
/// artist / album / title carry diacritics is found by the plain ASCII query
/// through the `remove_diacritics` FTS tokenizer (migration 0019), matching the
/// eval-side `fold`.
#[tokio::test]
async fn bare_text_folds_accents_through_fts() {
    use conservatory_core::db::{Album, Artist, Track};

    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();

    let artist = worker
        .insert_artist(Artist {
            id: 0,
            name: "Björk".into(),
            sort_name: "Björk".into(),
            musicbrainz_id: None,
        })
        .await
        .unwrap();
    let album = worker
        .insert_album(Album {
            id: 0,
            title: "Homogénic".into(),
            album_artist_id: Some(artist),
            shelf_genre: None,
            year: Some(1997),
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "x".into(),
            added_at: None,
        })
        .await
        .unwrap();
    let track = worker
        .insert_track(Track {
            id: 0,
            album_id: Some(album),
            artist_id: Some(artist),
            title: "Jóga".into(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: None,
            file_path: "x/1.flac".into(),
            format: Some("flac".into()),
            bitrate: None,
            sample_rate: None,
            replaygain_track: None,
            replaygain_album: None,
            rating: 0,
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: None,
        })
        .await
        .unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();

    // Bare ASCII text finds the accented artist / album / title through FTS.
    for q in ["bjork", "homogenic", "joga"] {
        let parsed = parse(q);
        let clause = try_translate(&parsed.expr, today)
            .unwrap_or_else(|| panic!("{q:?} should translate to an FTS clause"));
        let params: Vec<SqlParam> = clause.params.iter().map(to_param).collect();
        let ids = search_track_ids(&conn, &clause.sql, &params).unwrap();
        assert!(
            ids.contains(&track),
            "bare {q:?} should match the accented row via the folded FTS"
        );
    }

    worker.shutdown_ack().await.unwrap();
}
