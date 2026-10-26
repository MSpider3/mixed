//! Lyrics from online services: the LRCLIB API and the services that copy it,
//! Better Lyrics' community database (Unison) and LyricsPlus.

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

/// Better Lyrics' community lyrics; its main service needs a browser check.
const BETTER_LYRICS: Provider = Provider {
    name: "Better Lyrics",
    base: "https://unison.betterlyrics.org",
};

/// Collects lyrics from Apple Music, Musixmatch, QQ Music and others.
const LYRICS_PLUS: Provider = Provider {
    name: "LyricsPlus",
    base: "https://lyricsplus.prjktla.my.id",
};

/// Every source, in the order they are asked.
enum Source {
    /// Speaks the LRCLIB API
    Lrclib(&'static Provider),
    BetterLyrics,
    LyricsPlus,
}

const SOURCES: [Source; 4] = [
    Source::Lrclib(&PROVIDERS[0]),
    Source::Lrclib(&PROVIDERS[1]),
    Source::BetterLyrics,
    Source::LyricsPlus,
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

/// LRC timestamp for a time in milliseconds.
fn lrc_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}.{:02}",
        ms / 60_000,
        ms / 1000 % 60,
        ms / 10 % 100
    )
}

/// How closely LRC or plain text follows the music.
fn sync_of(text: &str) -> LyricsSync {
    if has_word_timing(text) {
        LyricsSync::Word
    } else if text
        .lines()
        .any(|l| l.starts_with('[') && l[1..].starts_with(|c: char| c.is_ascii_digit()))
    {
        LyricsSync::Line
    } else {
        LyricsSync::Plain
    }
}

/// A TTML time (`1:02.345`, `1:02:03.5` or `62.345s`) in milliseconds.
fn ttml_time(time: &str) -> Option<u64> {
    let seconds = match time.strip_suffix('s') {
        Some(seconds) => seconds.parse::<f64>().ok()?,
        None => time.split(':').try_fold(0.0, |total, part| {
            part.parse::<f64>().ok().map(|v| total * 60.0 + v)
        })?,
    };
    Some((seconds * 1000.0).round() as u64)
}

/// Convert TTML (as used by Apple Music and Better Lyrics) to LRC: a line per `<p>`,
/// with a time per word where its `<span>`s have one.
fn ttml_to_lrc(ttml: &str) -> Option<(String, LyricsSync)> {
    let line_re = regex::Regex::new(r#"(?s)<p\b[^>]*?\bbegin="([^"]+)"[^>]*>(.*?)</p>"#).ok()?;
    let word_re =
        regex::Regex::new(r#"(?s)<span\b[^>]*?\bbegin="([^"]+)"[^>]*>([^<]*)</span>"#).ok()?;
    let tag_re = regex::Regex::new(r"<[^>]*>").ok()?;
    let mut lrc = String::new();
    let mut words = false;
    for line in line_re.captures_iter(ttml) {
        let Some(start) = ttml_time(&line[1]) else {
            continue;
        };
        // Word times go in as \u{1}time\u{2} so that removing the tags keeps them
        let timed = word_re.replace_all(&line[2], |word: &regex::Captures| {
            words = true;
            match ttml_time(&word[1]) {
                Some(ms) => format!("\u{1}{}\u{2}{}", lrc_time(ms), &word[2]),
                None => word[2].to_string(),
            }
        });
        let text = tag_re
            .replace_all(&timed, "")
            .replace(['\n', '\r', '\t'], " ")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&#39;", "'")
            .replace('\u{1}', "<")
            .replace('\u{2}', ">");
        lrc.push_str(&format!("[{}]{}\n", lrc_time(start), text.trim()));
    }
    let sync = if words {
        LyricsSync::Word
    } else {
        LyricsSync::Line
    };
    (!lrc.is_empty()).then_some((lrc, sync))
}

#[derive(Debug, Deserialize)]
struct UnisonReply {
    data: UnisonLyrics,
}

#[derive(Debug, Deserialize)]
struct UnisonLyrics {
    song: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    lyrics: Option<String>,
    format: Option<String>,
}

/// The track's lyrics from Better Lyrics' community database.
async fn better_lyrics(
    http: &reqwest::Client,
    query: &LyricsQuery,
) -> Result<Option<LyricsCandidate>, String> {
    let duration = query.duration_secs.map(|d| d.to_string());
    let mut params = vec![
        ("song", query.title.as_str()),
        ("artist", query.artist.as_str()),
    ];
    if let Some(album) = query.album.as_deref() {
        params.push(("album", album));
    }
    if let Some(duration) = duration.as_deref() {
        params.push(("duration", duration));
    }
    let Some(reply) = get::<UnisonReply>(http, &BETTER_LYRICS, "/lyrics", &params).await? else {
        return Ok(None);
    };
    let found = reply.data;
    let Some(lyrics) = found.lyrics.filter(|l| !l.trim().is_empty()) else {
        return Ok(None);
    };
    let converted = if found.format.as_deref() == Some("ttml") {
        ttml_to_lrc(&lyrics)
    } else {
        let sync = sync_of(&lyrics);
        Some((lyrics, sync))
    };
    Ok(converted.map(|(text, sync)| LyricsCandidate {
        provider: BETTER_LYRICS.name,
        title: found.song.unwrap_or_else(|| query.title.clone()),
        artist: found.artist.unwrap_or_else(|| query.artist.clone()),
        album: found.album,
        duration_secs: query.duration_secs,
        sync,
        text,
    }))
}

#[derive(Debug, Deserialize)]
struct PlusReply {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    lyrics: Vec<PlusLine>,
}

#[derive(Debug, Deserialize)]
struct PlusLine {
    time: u64,
    text: String,
    #[serde(default)]
    syllabus: Vec<PlusSyllable>,
}

#[derive(Debug, Deserialize)]
struct PlusSyllable {
    time: u64,
    text: String,
}

/// Convert a LyricsPlus answer to LRC (with word times when it has them) or plain text.
fn plus_to_lrc(reply: &PlusReply) -> Option<(String, LyricsSync)> {
    let sync = match reply.kind.as_deref() {
        Some("Word") => LyricsSync::Word,
        Some("Line") => LyricsSync::Line,
        _ => LyricsSync::Plain,
    };
    let mut text = String::new();
    for line in &reply.lyrics {
        if sync != LyricsSync::Plain {
            text.push_str(&format!("[{}]", lrc_time(line.time)));
        }
        if sync == LyricsSync::Word && !line.syllabus.is_empty() {
            for syllable in &line.syllabus {
                // A blank syllable is the space between two words
                if !syllable.text.trim().is_empty() {
                    text.push_str(&format!("<{}>", lrc_time(syllable.time)));
                }
                text.push_str(&syllable.text);
            }
        } else {
            text.push_str(&line.text);
        }
        text.push('\n');
    }
    (!text.trim().is_empty()).then_some((text, sync))
}

/// The track's lyrics from LyricsPlus.
async fn lyrics_plus(
    http: &reqwest::Client,
    query: &LyricsQuery,
) -> Result<Option<LyricsCandidate>, String> {
    let duration = query.duration_secs.map(|d| d.to_string());
    let mut params = vec![
        ("title", query.title.as_str()),
        ("artist", query.artist.as_str()),
    ];
    if let Some(album) = query.album.as_deref() {
        params.push(("album", album));
    }
    if let Some(duration) = duration.as_deref() {
        params.push(("duration", duration));
    }
    let Some(reply) = get::<PlusReply>(http, &LYRICS_PLUS, "/v2/lyrics/get", &params).await? else {
        return Ok(None);
    };
    Ok(plus_to_lrc(&reply).map(|(text, sync)| LyricsCandidate {
        provider: LYRICS_PLUS.name,
        title: query.title.clone(),
        artist: query.artist.clone(),
        album: query.album.clone(),
        duration_secs: query.duration_secs,
        sync,
        text,
    }))
}

/// The lyrics a source has for the track.
async fn lookup_source(
    http: &reqwest::Client,
    source: &Source,
    query: &LyricsQuery,
) -> Result<Option<LyricsCandidate>, String> {
    match source {
        Source::Lrclib(provider) => lookup(http, provider, query).await,
        Source::BetterLyrics => better_lyrics(http, query).await,
        Source::LyricsPlus => lyrics_plus(http, query).await,
    }
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
    for source in &SOURCES {
        match lookup_source(http, source, query).await {
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
    for source in &SOURCES {
        let found = match (source, text) {
            (Source::Lrclib(provider), _) => search_provider(http, provider, query, text).await,
            // These look up a title and artist; they cannot search free text
            (_, Some(_)) => continue,
            (_, None) => lookup_source(http, source, query)
                .await
                .map(|found| found.into_iter().collect()),
        };
        match found {
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
    fn ttml_becomes_word_timed_lrc() {
        let ttml = r#"<tt><body><div>
            <p begin="0:33.642" end="0:35.909" itunes:key="L1"><span begin="0:33.642" end="0:33.958">Look</span> <span begin="0:33.958">at</span> <span begin="0:34.278">the &amp; stars</span></p>
            <p begin="1:02:03.5"><span ttm:role="x-bg"><span begin="62.5s">ooh</span></span></p>
            <p begin="0:40.000">a line without word times</p>
        </div></body></tt>"#;
        let (lrc, sync) = ttml_to_lrc(ttml).unwrap();
        assert_eq!(sync, LyricsSync::Word);
        let lines: Vec<_> = lrc.lines().collect();
        assert_eq!(
            lines[0],
            "[00:33.64]<00:33.64>Look <00:33.95>at <00:34.27>the & stars"
        );
        assert_eq!(lines[1], "[62:03.50]<01:02.50>ooh");
        assert_eq!(lines[2], "[00:40.00]a line without word times");
        // The app reads it back with its word times
        let parsed = crate::data::lyrics::parse_lrc_content(&lrc).unwrap();
        assert!(parsed.has_word_timestamps());
        assert_eq!(ttml_to_lrc("<tt></tt>"), None);
    }

    #[test]
    fn lyrics_plus_answers_become_lrc() {
        let reply: PlusReply = serde_json::from_str(
            r#"{"type":"Word","lyrics":[{"time":2438,"duration":1,"text":"Raabta ho",
                "syllabus":[{"time":2438,"duration":1,"text":"Raabta"},{"time":3000,"duration":1,"text":" "},{"time":3100,"duration":1,"text":"ho"}]}]}"#,
        )
        .unwrap();
        let (lrc, sync) = plus_to_lrc(&reply).unwrap();
        assert_eq!(sync, LyricsSync::Word);
        assert_eq!(lrc, "[00:02.43]<00:02.43>Raabta <00:03.10>ho\n");

        let reply: PlusReply = serde_json::from_str(
            r#"{"type":"Line","lyrics":[{"time":35420,"duration":3250,"text":"Look at the stars"}]}"#,
        )
        .unwrap();
        assert_eq!(
            plus_to_lrc(&reply),
            Some(("[00:35.42]Look at the stars\n".into(), LyricsSync::Line))
        );
        let empty: PlusReply = serde_json::from_str(r#"{"type":"Line","lyrics":[]}"#).unwrap();
        assert_eq!(plus_to_lrc(&empty), None);
    }

    #[test]
    fn first_artist_of_a_joined_list() {
        assert_eq!(first_artist("A, B, C"), "A");
        assert_eq!(first_artist("A; B"), "A");
        assert_eq!(first_artist("Solo"), "Solo");
    }
}
