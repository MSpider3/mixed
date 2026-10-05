//! Where lyrics found online are kept, and where a track's lyrics are looked up.

use std::path::{Path, PathBuf};

use crate::data::lyrics::{self, Lyrics};
use crate::data::track::TrackRef;

/// First line of every lyrics file mixed writes. A lyrics file without it was made
/// by the user and is not replaced without asking.
pub const APP_MARKER: &str = "[re:mixed]";

fn cache_dir() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join("mixed_test_lyrics")
    }
    #[cfg(not(test))]
    {
        directories::ProjectDirs::from("", "", "mixed")
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir)
            .join("lyrics")
    }
}

/// File name (without extension) of a track's entries in the lyrics cache.
fn cache_key(track: &TrackRef) -> String {
    let id_chars = |id: &str| -> String {
        id.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    };
    match track {
        TrackRef::Local(path) => format!(
            "local_{}",
            crate::utils::hash::fnv1a_hex(&path.to_string_lossy())
        ),
        TrackRef::Spotify(uri) => format!(
            "spotify_{}",
            id_chars(uri.strip_prefix("spotify:track:").unwrap_or(uri))
        ),
        TrackRef::YouTube(id) => format!("youtube_{}", id_chars(id)),
    }
}

fn cached_path(track: &TrackRef) -> PathBuf {
    cache_dir().join(format!("{}.lrc", cache_key(track)))
}

/// Marks a track for which a search found nothing, so it is not searched on every play.
fn none_marker(track: &TrackRef) -> PathBuf {
    cache_dir().join(format!("{}.none", cache_key(track)))
}

/// The `.lrc` file next to a local track.
fn adjacent_path(track: &TrackRef) -> Option<PathBuf> {
    track.local_path().map(|p| p.with_extension("lrc"))
}

fn is_app_file(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|content| content.trim_start().starts_with(APP_MARKER))
}

/// The lyrics a track has without going online: what mixed saved in its cache, the
/// `.lrc` file next to a local track, or the lyrics embedded in its tags.
pub fn load(track: &TrackRef) -> Option<Lyrics> {
    lyrics::read_lyrics_file(&cached_path(track))
        .or_else(|| adjacent_path(track).and_then(|p| lyrics::read_lyrics_file(&p)))
        .or_else(|| track.local_path().and_then(lyrics::load_embedded_lyrics))
}

/// True if an earlier search found no lyrics for the track.
pub fn is_marked_none(track: &TrackRef) -> bool {
    none_marker(track).exists()
}

/// The `.lrc` file next to a local track, if the user made it (mixed did not write it).
pub fn user_file(track: &TrackRef) -> Option<PathBuf> {
    adjacent_path(track).filter(|p| p.exists() && !is_app_file(p))
}

/// Save lyrics for a track and return where they went: next to a local track when
/// `beside_song` is set, otherwise (or if that folder cannot be written) in the cache.
pub fn save(track: &TrackRef, text: &str, beside_song: bool) -> std::io::Result<PathBuf> {
    let content = format!("{}\n{}\n", APP_MARKER, text.trim_end());
    let _ = std::fs::remove_file(none_marker(track));
    let cached = cached_path(track);

    if let Some(adjacent) = adjacent_path(track).filter(|_| beside_song) {
        if std::fs::write(&adjacent, &content).is_ok() {
            // The cache is read first: an older copy there must not hide this file
            let _ = std::fs::remove_file(&cached);
            return Ok(adjacent);
        }
    }
    std::fs::create_dir_all(cache_dir())?;
    std::fs::write(&cached, content)?;
    Ok(cached)
}

/// Remove what mixed saved for a track. A lyrics file the user made is left alone.
pub fn forget(track: &TrackRef) {
    let _ = std::fs::remove_file(cached_path(track));
    let _ = std::fs::remove_file(none_marker(track));
    if let Some(adjacent) = adjacent_path(track).filter(|p| is_app_file(p)) {
        let _ = std::fs::remove_file(adjacent);
    }
}

/// Remember that the track has no lyrics, removing any that mixed saved for it.
pub fn mark_none(track: &TrackRef) {
    forget(track);
    if std::fs::create_dir_all(cache_dir()).is_ok() {
        let _ = std::fs::write(none_marker(track), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNCED: &str = "[00:01.00]first line\n[00:02.50]second line";

    /// A unique fake track in a folder of its own.
    fn local_track(name: &str) -> TrackRef {
        let dir = std::env::temp_dir().join(format!("mixed_test_lyrics_songs_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TrackRef::Local(dir.join("song.mp3"))
    }

    #[test]
    fn lyrics_are_saved_next_to_a_local_song_and_found_again() {
        let track = local_track("beside");
        let path = save(&track, SYNCED, true).unwrap();
        assert_eq!(path, track.local_path().unwrap().with_extension("lrc"));
        assert!(matches!(load(&track), Some(Lyrics::Synced(data)) if data.lines.len() == 2));
        assert_eq!(
            user_file(&track),
            None,
            "a file mixed wrote may be replaced"
        );

        forget(&track);
        assert!(!path.exists());
        assert!(load(&track).is_none());
    }

    #[test]
    fn lyrics_go_to_the_cache_when_not_saved_next_to_the_song() {
        let track = local_track("cache");
        let path = save(&track, "just words\n\nmore words", false).unwrap();
        assert!(path.starts_with(cache_dir()));
        assert!(!track.local_path().unwrap().with_extension("lrc").exists());
        assert!(matches!(load(&track), Some(Lyrics::Plain(lines)) if lines.len() == 3));
        forget(&track);
    }

    #[test]
    fn a_lyrics_file_made_by_the_user_is_recognised_and_kept() {
        let track = local_track("own");
        let own = track.local_path().unwrap().with_extension("lrc");
        std::fs::write(&own, SYNCED).unwrap();
        assert_eq!(user_file(&track), Some(own.clone()));

        mark_none(&track);
        assert!(own.exists(), "only files mixed wrote are removed");
        assert!(is_marked_none(&track));
        forget(&track);
        assert!(!is_marked_none(&track));
    }

    #[test]
    fn streamed_tracks_use_the_cache_and_remember_a_miss() {
        let track = TrackRef::Spotify("spotify:track:TestLyricsStore01".into());
        forget(&track);
        assert!(load(&track).is_none());

        mark_none(&track);
        assert!(is_marked_none(&track));

        let path = save(&track, SYNCED, true).unwrap();
        assert!(path.ends_with("spotify_TestLyricsStore01.lrc"));
        assert!(!is_marked_none(&track), "found lyrics replace the miss");
        assert!(load(&track).is_some());
        forget(&track);
    }
}
