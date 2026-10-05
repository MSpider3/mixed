//! Lyrics of the playing track: finding them, keeping them, and choosing others.

use std::path::PathBuf;

use super::App;
use crate::data::lyrics::{parse_lyrics_text, Lyrics};
use crate::data::lyrics_store;
use crate::data::track::TrackRef;
use crate::sources::{LyricsCandidate, LyricsQuery, SourceRequest};

/// Rows of the picker above the matches: "Automatic" and "No lyrics".
pub const PICKER_FIXED_ROWS: usize = 2;

/// The list of other lyrics for the playing track, opened from the Now Playing view.
#[derive(Debug, Clone)]
pub struct LyricsPicker {
    pub track: TrackRef,
    pub matches: Vec<LyricsCandidate>,
    /// Selected row: 0 is "Automatic", 1 is "No lyrics", then the matches.
    pub cursor: usize,
    pub loading: bool,
    pub error: Option<String>,
    /// The user's own search words, while they are being typed.
    pub input: Option<String>,
    /// A chosen match that would replace this lyrics file, which the user made.
    pub confirm_replace: Option<(LyricsCandidate, PathBuf)>,
}

impl App {
    fn is_current_track(&self, track: &TrackRef) -> bool {
        self.playlist
            .current_entry()
            .is_some_and(|e| &e.id == track)
    }

    /// What the playing track's lyrics are looked up by. `None` for a track without a title.
    fn lyrics_query(&self) -> Option<LyricsQuery> {
        let meta = &self.playlist.current_entry()?.metadata;
        if meta.title.is_none() && meta.sanitized_title.is_none() {
            return None;
        }
        Some(LyricsQuery {
            title: meta
                .display_title(self.config.strip_track_numbers)
                .to_string(),
            artist: meta.artist.clone().unwrap_or_default(),
            album: meta.album.clone(),
            duration_secs: meta.duration.map(|d| d.as_secs()),
        })
    }

    pub(super) fn clear_lyrics(&mut self) {
        self.current_lyrics = None;
        self.current_plain_lyrics = None;
        self.lyrics_searching = false;
        self.lyrics_picker = None;
    }

    fn show_lyrics(&mut self, lyrics: Lyrics) {
        match lyrics {
            Lyrics::Synced(data) => {
                self.current_lyrics = Some(data);
                self.current_plain_lyrics = None;
            }
            Lyrics::Plain(lines) => {
                self.current_lyrics = None;
                self.current_plain_lyrics = Some(lines);
            }
        }
        self.lyrics_searching = false;
        self.lyrics_scroll = 0;
        self.refresh_needed = true;
    }

    /// Show the lyrics of the track that starts playing: the ones it already has
    /// (saved, next to the file, or in its tags), else what a search online finds.
    pub(super) fn load_lyrics_for_current(&mut self) {
        self.clear_lyrics();
        let Some(track) = self.playlist.current_entry().map(|e| e.id.clone()) else {
            return;
        };
        if let Some(lyrics) = lyrics_store::load(&track) {
            self.show_lyrics(lyrics);
        } else if !lyrics_store::is_marked_none(&track) {
            // Spotify has lyrics of its own for its tracks: those are asked for first
            match (&track, self.player.as_ref()) {
                (TrackRef::Spotify(uri), Some(player)) => {
                    player.fetch_spotify_lyrics(uri.clone());
                    self.lyrics_searching = true;
                }
                _ => self.ask_lyrics_providers(track),
            }
        }
    }

    /// Ask the lyrics providers for the playing track's lyrics.
    fn ask_lyrics_providers(&mut self, track: TrackRef) {
        self.lyrics_searching = false;
        if let (Some(rt), Some(query)) = (self.source_runtime.as_ref(), self.lyrics_query()) {
            let _ = rt.send(SourceRequest::FetchLyrics {
                track,
                query: Box::new(query),
            });
            self.lyrics_searching = true;
        }
        self.refresh_needed = true;
    }

    /// Save lyrics for a track and show them.
    fn keep_lyrics(&mut self, track: &TrackRef, provider: &str, text: &str) {
        let Some(lyrics) = parse_lyrics_text(text) else {
            self.lyrics_searching = false;
            return;
        };
        match lyrics_store::save(track, text, self.config.save_lyrics_next_to_songs) {
            Ok(path) => log::info!("Saved lyrics from {} to {}", provider, path.display()),
            Err(e) => log::warn!("Could not save lyrics: {}", e),
        }
        self.show_lyrics(lyrics);
        self.set_status(format!("Lyrics from {}", provider));
    }

    /// Spotify answered a lyrics request for one of its tracks.
    pub(super) fn handle_spotify_lyrics(&mut self, uri: String, lyrics: Option<String>) {
        let track = TrackRef::Spotify(uri);
        // An answer that comes after the track changed, or after the user chose
        // lyrics by hand, is no longer wanted
        if !(self.lyrics_searching && self.is_current_track(&track)) {
            return;
        }
        match lyrics {
            Some(text) => self.keep_lyrics(&track, "Spotify", &text),
            None => self.ask_lyrics_providers(track),
        }
    }

    /// The lyrics providers answered the search for a track's lyrics.
    pub(super) fn handle_lyrics_fetched(
        &mut self,
        track: TrackRef,
        result: Result<Option<LyricsCandidate>, String>,
    ) {
        if !(self.lyrics_searching && self.is_current_track(&track)) {
            return;
        }
        match result {
            Ok(Some(found)) => self.keep_lyrics(&track, found.provider, &found.text),
            Ok(None) => {
                // Remembered, so that the track is not searched on every play
                lyrics_store::mark_none(&track);
                self.lyrics_searching = false;
            }
            Err(e) => {
                log::warn!("Lyrics search failed: {}", e);
                self.lyrics_searching = false;
            }
        }
        self.refresh_needed = true;
    }

    /// Open the list of other lyrics for the playing track and search for them.
    pub fn open_lyrics_picker(&mut self) {
        let Some(track) = self.playlist.current_entry().map(|e| e.id.clone()) else {
            return;
        };
        self.lyrics_picker = Some(LyricsPicker {
            track,
            matches: Vec::new(),
            cursor: 0,
            loading: false,
            error: None,
            input: None,
            confirm_replace: None,
        });
        self.show_full_lyrics = true;
        self.search_lyrics_matches(None);
    }

    /// Fill the picker with matches for the track, or for the user's own words.
    pub fn search_lyrics_matches(&mut self, text: Option<String>) {
        let query = self.lyrics_query().unwrap_or_default();
        let Some(picker) = self.lyrics_picker.as_mut() else {
            return;
        };
        picker.matches.clear();
        picker.cursor = picker.cursor.min(PICKER_FIXED_ROWS - 1);
        picker.error = None;
        picker.loading = false;
        match self.source_runtime.as_ref() {
            _ if text.is_none() && query.title.is_empty() => {
                picker.error = Some("This track has no title: press / to search by hand".into());
            }
            Some(rt) => {
                let _ = rt.send(SourceRequest::SearchLyrics {
                    track: picker.track.clone(),
                    query: Box::new(query),
                    text,
                });
                picker.loading = true;
            }
            None => picker.error = Some("Online lyrics are not available".into()),
        }
        self.refresh_needed = true;
    }

    /// The lyrics providers answered a search of the picker.
    pub(super) fn handle_lyrics_matches(
        &mut self,
        track: TrackRef,
        result: Result<Vec<LyricsCandidate>, String>,
    ) {
        let Some(picker) = self.lyrics_picker.as_mut().filter(|p| p.track == track) else {
            return;
        };
        picker.loading = false;
        match result {
            Ok(matches) => picker.matches = matches,
            Err(e) => picker.error = Some(e),
        }
        self.refresh_needed = true;
    }

    pub fn move_lyrics_cursor(&mut self, down: bool) {
        if let Some(picker) = self.lyrics_picker.as_mut() {
            let last = PICKER_FIXED_ROWS + picker.matches.len() - 1;
            picker.cursor = if down {
                (picker.cursor + 1).min(last)
            } else {
                picker.cursor.saturating_sub(1)
            };
            self.refresh_needed = true;
        }
    }

    /// Act on the selected row of the picker.
    pub fn choose_lyrics(&mut self) {
        let Some(picker) = self.lyrics_picker.as_mut() else {
            return;
        };
        let track = picker.track.clone();
        match picker.cursor {
            0 => {
                lyrics_store::forget(&track);
                self.load_lyrics_for_current();
            }
            1 => {
                lyrics_store::mark_none(&track);
                self.load_lyrics_for_current();
                self.set_status(if self.has_lyrics() {
                    "Saved lyrics removed; the song's own lyrics are kept"
                } else {
                    "No lyrics for this track"
                });
            }
            row => {
                let Some(found) = picker.matches.get(row - PICKER_FIXED_ROWS).cloned() else {
                    return;
                };
                // Saving next to the song would overwrite a lyrics file the user made
                let own = lyrics_store::user_file(&track)
                    .filter(|_| self.config.save_lyrics_next_to_songs);
                match own {
                    Some(path) => picker.confirm_replace = Some((found, path)),
                    None => {
                        self.lyrics_picker = None;
                        self.keep_lyrics(&track, found.provider, &found.text);
                    }
                }
            }
        }
        self.refresh_needed = true;
    }

    /// Answer the question whether the user's own lyrics file may be replaced.
    pub fn confirm_lyrics_replace(&mut self, replace: bool) {
        let Some(picker) = self.lyrics_picker.as_mut() else {
            return;
        };
        if let Some((found, _)) = picker.confirm_replace.take() {
            if replace {
                let track = picker.track.clone();
                self.lyrics_picker = None;
                self.keep_lyrics(&track, found.provider, &found.text);
            }
        }
        self.refresh_needed = true;
    }

    fn has_lyrics(&self) -> bool {
        self.current_lyrics.is_some() || self.current_plain_lyrics.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::app_config::AppConfig;
    use crate::data::metadata::TrackMetadata;
    use crate::sources::{LyricsSync, SourceEvent};

    const LRC: &str = "[00:01.00]first line\n[00:02.00]second line";

    /// An app whose playing track is `track`.
    fn app_playing(track: &TrackRef) -> App {
        let (mpris_tx, _mpris_rx) = crossbeam_channel::bounded(8);
        let (vis_tx, _vis_rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(AppConfig::default(), mpris_tx, vis_tx);
        let meta = TrackMetadata {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            ..Default::default()
        };
        app.playlist.add(track.clone(), meta);
        assert!(app.is_current_track(track));
        lyrics_store::forget(track);
        app
    }

    fn found(text: &str) -> LyricsCandidate {
        LyricsCandidate {
            provider: "LRCLIB",
            title: "Song".into(),
            artist: "Artist".into(),
            album: None,
            duration_secs: Some(180),
            sync: LyricsSync::Line,
            text: text.into(),
        }
    }

    /// Open the picker with one match and move the cursor onto it.
    fn pick_first_match(app: &mut App, track: &TrackRef) {
        app.open_lyrics_picker();
        app.handle_source_event(SourceEvent::LyricsMatches {
            track: track.clone(),
            result: Ok(vec![found(LRC)]),
        });
        app.move_lyrics_cursor(true);
        app.move_lyrics_cursor(true);
        app.move_lyrics_cursor(true);
        assert_eq!(
            app.lyrics_picker.as_ref().unwrap().cursor,
            PICKER_FIXED_ROWS
        );
        app.choose_lyrics();
    }

    #[test]
    fn a_chosen_match_is_saved_and_shown() {
        let track = TrackRef::Spotify("spotify:track:TestPickerChoice01".into());
        let mut app = app_playing(&track);

        pick_first_match(&mut app, &track);
        assert!(app.lyrics_picker.is_none());
        assert_eq!(app.current_lyrics.as_ref().map(|l| l.lines.len()), Some(2));
        assert!(
            lyrics_store::load(&track).is_some(),
            "kept for the next play"
        );

        // "No lyrics" removes them again and is remembered
        app.open_lyrics_picker();
        app.move_lyrics_cursor(true);
        app.choose_lyrics();
        assert!(app.current_lyrics.is_none());
        assert!(lyrics_store::is_marked_none(&track));
        lyrics_store::forget(&track);
    }

    #[test]
    fn the_users_own_lyrics_file_is_only_replaced_after_confirming() {
        let dir = std::env::temp_dir().join("mixed_test_lyrics_picker_own");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let track = TrackRef::Local(dir.join("song.mp3"));
        let own = dir.join("song.lrc");
        let mut app = app_playing(&track);
        std::fs::write(&own, "[00:05.00]my own words").unwrap();

        pick_first_match(&mut app, &track);
        assert!(app
            .lyrics_picker
            .as_ref()
            .unwrap()
            .confirm_replace
            .is_some());
        app.confirm_lyrics_replace(false);
        assert_eq!(
            std::fs::read_to_string(&own).unwrap(),
            "[00:05.00]my own words"
        );
        assert!(app.lyrics_picker.is_some(), "the list stays open");

        app.choose_lyrics();
        app.confirm_lyrics_replace(true);
        assert!(app.lyrics_picker.is_none());
        assert!(std::fs::read_to_string(&own)
            .unwrap()
            .starts_with(lyrics_store::APP_MARKER));
        assert_eq!(app.current_lyrics.as_ref().map(|l| l.lines.len()), Some(2));
    }

    #[test]
    fn search_answers_apply_only_to_the_track_they_were_asked_for() {
        let track = TrackRef::YouTube("TestLyricsAnswer01".into());
        let other = TrackRef::YouTube("TestLyricsAnswer02".into());
        let mut app = app_playing(&track);
        lyrics_store::forget(&other);
        app.lyrics_searching = true;

        // An answer for a track that is no longer playing changes nothing
        app.handle_lyrics_fetched(other.clone(), Ok(None));
        assert!(!lyrics_store::is_marked_none(&other));
        assert!(app.lyrics_searching);

        // A search that could not be made is not remembered as "no lyrics"
        app.handle_lyrics_fetched(track.clone(), Err("offline".into()));
        assert!(!lyrics_store::is_marked_none(&track));
        assert!(!app.lyrics_searching);

        app.lyrics_searching = true;
        app.handle_lyrics_fetched(track.clone(), Ok(None));
        assert!(lyrics_store::is_marked_none(&track));

        // Plain text is shown when nothing synced exists
        app.lyrics_searching = true;
        app.handle_lyrics_fetched(track.clone(), Ok(Some(found("just words\nmore words"))));
        assert_eq!(app.current_plain_lyrics.as_ref().map(Vec::len), Some(2));
        assert!(!lyrics_store::is_marked_none(&track));
        lyrics_store::forget(&track);
    }
}
