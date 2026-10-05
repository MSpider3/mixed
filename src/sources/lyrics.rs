//! Lyrics from online services that speak the LRCLIB API.

use super::{LyricsCandidate, LyricsQuery, LyricsSync};
use serde::Deserialize;

struct Provider {
    name: &'static str,
    base: &'static str,
}

/// Asked in this order. Karalyr only has word-synced lyrics, so what it has is
/// preferred over the line-synced lyrics LRCLIB has for far more songs.
const PROVIDERS: [Provider; 2] = [
    Provider {
        name: "Karalyr",
        base: "https://www.karalyr.com",
    },
    Provider {
        name: "LRCLIB",
        base: "https://lrclib.net",
    },
];

const USER_AGENT: &str = concat!("mixed/", env!("CARGO_PKG_VERSION"));

/// A match may differ from the track by this many seconds and still be the same recording.
const DURATION_TOLERANCE_SECS: u64 = 3;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

/// True if LRC text carries a time for each word (`<mm:ss.xx>` tags).
fn has_word_timing(lrc: &str) -> bool {
    lrc.split('<').skip(1).any(|rest| {
        let tag = rest.split('>').next().unwrap_or("");
        tag.contains(':') && tag.starts_with(|c: char| c.is_ascii_digit())
    })
}

impl Record {
    /// The record as a candidate, or `None` if it holds no lyrics (an instrumental).
    fn into_candidate(self, provider: &'static str) -> Option<LyricsCandidate> {
        let filled = |text: Option<String>| text.filter(|t| !t.trim().is_empty());
        let (sync, text) = match (filled(self.synced_lyrics), filled(self.plain_lyrics)) {
            (Some(lrc), _) if has_word_timing(&lrc) => (LyricsSync::Word, lrc),
            (Some(lrc), _) => (LyricsSync::Line, lrc),
            (None, Some(plain)) => (LyricsSync::Plain, plain),
            (None, None) => return None,
        };
        Some(LyricsCandidate {
            provider,
            title: self.track_name.unwrap_or_default(),
            artist: self.artist_name.unwrap_or_default(),
            album: self.album_name,
            duration_secs: self.duration.map(|d| d.round() as u64),
            sync,
            text,
        })
    }
}

/// How far a candidate's length is from the track's, if both are known.
fn duration_gap(candidate: &LyricsCandidate, query: &LyricsQuery) -> Option<u64> {
    Some(candidate.duration_secs?.abs_diff(query.duration_secs?))
}

/// True unless the lengths are known and too far apart to be the same recording.
fn fits_duration(candidate: &LyricsCandidate, query: &LyricsQuery) -> bool {
    duration_gap(candidate, query).is_none_or(|gap| gap <= DURATION_TOLERANCE_SECS)
}

/// Order matches for the track: those of its length first, the better synced
/// before the rest, then by how close the length is.
fn rank(candidates: &mut [LyricsCandidate], query: &LyricsQuery) {
    candidates.sort_by_key(|c| {
        (
            !fits_duration(c, query),
            std::cmp::Reverse(c.sync),
            duration_gap(c, query).unwrap_or(0),
        )
    });
}

/// The first of several artists, which is how providers usually file a track.
fn first_artist(artist: &str) -> &str {
    artist.split([',', ';']).next().unwrap_or(artist).trim()
}

/// GET a provider URL. `Ok(None)` means the provider answered "not found".
async fn get<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    provider: &Provider,
    path: &str,
    params: &[(&str, &str)],
) -> Result<Option<T>, String> {
    let resp = http
        .get(format!("{}{}", provider.base, path))
        .query(params)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("{}: {}", provider.name, e))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("{}: HTTP {}", provider.name, resp.status()));
    }
    resp.json()
        .await
        .map(Some)
        .map_err(|e| format!("{}: {}", provider.name, e))
}

async fn search_provider(
    http: &reqwest::Client,
    provider: &Provider,
    query: &LyricsQuery,
    text: Option<&str>,
) -> Result<Vec<LyricsCandidate>, String> {
    let params = match text {
        Some(text) => vec![("q", text)],
        None => vec![
            ("track_name", query.title.as_str()),
            ("artist_name", first_artist(&query.artist)),
        ],
    };
    let records: Vec<Record> = get(http, provider, "/api/search", &params)
        .await?
        .unwrap_or_default();
    Ok(records
        .into_iter()
        .filter_map(|r| r.into_candidate(provider.name))
        .collect())
}

/// The provider's lyrics for the track: its exact entry, else the best search match
/// of the same length.
async fn lookup(
    http: &reqwest::Client,
    provider: &Provider,
    query: &LyricsQuery,
) -> Result<Option<LyricsCandidate>, String> {
    let duration = query.duration_secs.map(|d| d.to_string());
    let mut params = vec![
        ("track_name", query.title.as_str()),
        ("artist_name", query.artist.as_str()),
    ];
    if let Some(album) = query.album.as_deref() {
        params.push(("album_name", album));
    }
    if let Some(duration) = duration.as_deref() {
        params.push(("duration", duration));
    }
    // A provider too busy for this request may still answer the search below
    let exact = get::<Record>(http, provider, "/api/get", &params)
        .await
        .unwrap_or_else(|e| {
            log::debug!("{}", e);
            None
        })
        .and_then(|r| r.into_candidate(provider.name));
    if exact.is_some() {
        return Ok(exact);
    }

    let mut matches = search_provider(http, provider, query, None).await?;
    matches.retain(|c| fits_duration(c, query));
    rank(&mut matches, query);
    Ok(matches.into_iter().next())
}

/// Find lyrics for a track: the first provider's synced lyrics, or failing those
/// the first plain ones. `Ok(None)` if every provider answered and none has any.
pub async fn find(
    http: &reqwest::Client,
    query: &LyricsQuery,
) -> Result<Option<LyricsCandidate>, String> {
    let mut plain = None;
    let mut error = None;
    for provider in &PROVIDERS {
        match lookup(http, provider, query).await {
            Ok(Some(found)) if found.sync > LyricsSync::Plain => return Ok(Some(found)),
            Ok(Some(found)) => plain = plain.or(Some(found)),
            Ok(None) => {}
            Err(e) => {
                log::warn!("Lyrics lookup failed: {}", e);
                error = Some(e);
            }
        }
    }
    match (plain, error) {
        (Some(found), _) => Ok(Some(found)),
        // A provider that could not be asked might have had them
        (None, Some(e)) => Err(e),
        (None, None) => Ok(None),
    }
}

/// Every match of all providers, best first. With `text` the search uses the
/// user's own words instead of the track's title and artist.
pub async fn search(
    http: &reqwest::Client,
    query: &LyricsQuery,
    text: Option<&str>,
) -> Result<Vec<LyricsCandidate>, String> {
    let mut matches = Vec::new();
    let mut error = None;
    for provider in &PROVIDERS {
        match search_provider(http, provider, query, text).await {
            Ok(found) => matches.extend(found),
            Err(e) => {
                log::warn!("Lyrics search failed: {}", e);
                error = Some(e);
            }
        }
    }
    match error {
        Some(e) if matches.is_empty() => Err(e),
        _ => {
            rank(&mut matches, query);
            Ok(matches)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(json: &str) -> Record {
        serde_json::from_str(json).unwrap()
    }

    fn candidate(sync: LyricsSync, duration_secs: u64) -> LyricsCandidate {
        LyricsCandidate {
            provider: "test",
            title: String::new(),
            artist: String::new(),
            album: None,
            duration_secs: Some(duration_secs),
            sync,
            text: String::new(),
        }
    }

    #[test]
    fn records_become_candidates_with_their_best_lyrics() {
        let word = record(
            r#"{"trackName":"Song","artistName":"A","albumName":null,"duration":223.4,
                "plainLyrics":"one two","syncedLyrics":"[00:01.00]<00:01.00>one <00:01.50>two"}"#,
        )
        .into_candidate("Karalyr")
        .unwrap();
        assert_eq!(word.sync, LyricsSync::Word);
        assert_eq!(word.duration_secs, Some(223));
        assert_eq!(word.album, None);

        let line = record(r#"{"plainLyrics":"one","syncedLyrics":"[00:01.00]one <3"}"#)
            .into_candidate("LRCLIB")
            .unwrap();
        assert_eq!(line.sync, LyricsSync::Line);

        let plain = record(r#"{"plainLyrics":"one","syncedLyrics":null}"#)
            .into_candidate("LRCLIB")
            .unwrap();
        assert_eq!(
            (plain.sync, plain.text.as_str()),
            (LyricsSync::Plain, "one")
        );

        // An instrumental has an entry but no lyrics
        assert!(
            record(r#"{"trackName":"Song","plainLyrics":"","syncedLyrics":null}"#)
                .into_candidate("LRCLIB")
                .is_none()
        );
    }

    #[test]
    fn matches_of_the_tracks_length_and_better_sync_come_first() {
        let query = LyricsQuery {
            duration_secs: Some(200),
            ..Default::default()
        };
        let mut matches = vec![
            candidate(LyricsSync::Word, 260),
            candidate(LyricsSync::Plain, 200),
            candidate(LyricsSync::Line, 202),
            candidate(LyricsSync::Line, 200),
        ];
        rank(&mut matches, &query);
        let order: Vec<_> = matches
            .iter()
            .map(|c| (c.sync, c.duration_secs.unwrap()))
            .collect();
        assert_eq!(
            order,
            vec![
                (LyricsSync::Line, 200),
                (LyricsSync::Line, 202),
                (LyricsSync::Plain, 200),
                (LyricsSync::Word, 260),
            ]
        );
        assert!(!fits_duration(&matches[3], &query));
        // Nothing to compare when the track's length is unknown
        assert!(fits_duration(&matches[3], &LyricsQuery::default()));
    }

    #[test]
    fn first_artist_of_a_joined_list() {
        assert_eq!(first_artist("A, B, C"), "A");
        assert_eq!(first_artist("A; B"), "A");
        assert_eq!(first_artist("Solo"), "Solo");
    }
}
