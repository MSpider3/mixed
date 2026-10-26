//! Spotify's catalog as the desktop app sees it: radio for a track and the tracks of
//! any playlist, including the ones Spotify makes (Daily Mix, Discover Weekly, radio).
//!
//! Since November 2024 the Web API refuses these to apps like mixed (algorithmic and
//! Spotify-owned playlists answer 404, recommendations are gone). librespot talks to
//! Spotify like the desktop app does, so it still gets them.

use librespot_core::session::Session;
use librespot_core::spotify_uri::SpotifyUri;
use librespot_metadata::{Metadata, Track};

use crate::data::track::TrackRef;
use crate::sources::{BrowseItem, BrowseItemKind};

/// How many tracks of a playlist are listed at most.
const MAX_TRACKS: usize = 300;
/// How many track details are asked for at the same time.
const PARALLEL_LOOKUPS: usize = 16;

/// A librespot session for catalog requests, connected when first needed.
#[derive(Default)]
pub struct SpotifyCatalog {
    session: tokio::sync::Mutex<Option<Session>>,
}

impl SpotifyCatalog {
    async fn session(&self) -> Result<Session, String> {
        let mut slot = self.session.lock().await;
        if let Some(session) = slot.as_ref().filter(|s| !s.is_invalid()) {
            return Ok(session.clone());
        }
        let session = crate::audio::spotify_backend::catalog_session().await?;
        *slot = Some(session.clone());
        Ok(session)
    }

    /// Tracks of a playlist, album or other context URI, with their details.
    pub async fn context_tracks(&self, uri: &str) -> Result<Vec<BrowseItem>, String> {
        let session = self.session().await?;
        let context = session
            .spclient()
            .get_context(uri)
            .await
            .map_err(|e| format!("Spotify could not open {}: {}", uri, e))?;
        let uris: Vec<String> = context
            .pages
            .iter()
            .flat_map(|page| page.tracks.iter())
            .map(|track| track.uri().to_string())
            .filter(|uri| uri.starts_with("spotify:track:"))
            .take(MAX_TRACKS)
            .collect();
        Ok(track_items(&session, uris).await)
    }

    /// Spotify's radio for a track: the playlist it makes from it, as tracks.
    pub async fn radio(&self, track_uri: &str) -> Result<Vec<BrowseItem>, String> {
        let session = self.session().await?;
        let seed = SpotifyUri::from_uri(track_uri).map_err(|e| e.to_string())?;
        let answer = session
            .spclient()
            .get_radio_for_track(&seed)
            .await
            .map_err(|e| format!("Spotify has no radio for this track: {}", e))?;
        let playlist = radio_playlist(&answer).ok_or("Spotify has no radio for this track")?;
        drop(session);
        self.context_tracks(&playlist).await
    }
}

/// The playlist URI in an answer of Spotify's radio service.
fn radio_playlist(answer: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(answer).ok()?;
    value
        .get("mediaItems")?
        .as_array()?
        .iter()
        .filter_map(|item| item.get("uri")?.as_str())
        .find(|uri| uri.starts_with("spotify:playlist:"))
        .map(str::to_string)
}

/// Details of tracks, in the order given; tracks Spotify cannot describe are left out.
async fn track_items(session: &Session, uris: Vec<String>) -> Vec<BrowseItem> {
    let mut found: Vec<Option<BrowseItem>> = vec![None; uris.len()];
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = uris.into_iter().enumerate();
    loop {
        while tasks.len() < PARALLEL_LOOKUPS {
            let Some((index, uri)) = pending.next() else {
                break;
            };
            let session = session.clone();
            tasks.spawn(async move { (index, track_item(&session, &uri).await) });
        }
        match tasks.join_next().await {
            Some(Ok((index, item))) => found[index] = item,
            Some(Err(_)) => {}
            None => break,
        }
    }
    found.into_iter().flatten().collect()
}

async fn track_item(session: &Session, uri: &str) -> Option<BrowseItem> {
    let id = SpotifyUri::from_uri(uri).ok()?;
    let track = match Track::get(session, &id).await {
        Ok(track) => track,
        Err(e) => {
            log::debug!("No details for {}: {}", uri, e);
            return None;
        }
    };
    let artists: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
    // The biggest cover, as an image URL
    let cover = track
        .album
        .covers
        .iter()
        .max_by_key(|image| image.width)
        .and_then(|image| image.id.to_base16().ok())
        .map(|hex| format!("https://i.scdn.co/image/{}", hex));
    Some(BrowseItem {
        id: uri.to_string(),
        title: track.name,
        subtitle: Some(artists.join(", ")),
        kind: BrowseItemKind::Track,
        is_container: false,
        track_ref: Some(TrackRef::Spotify(uri.to_string())),
        duration_secs: Some(u64::try_from(track.duration).unwrap_or(0) / 1000),
        artwork_url: cover,
        album: Some(track.album.name).filter(|n| !n.is_empty()),
        depth: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_radio_answer_names_its_playlist() {
        let answer = br#"{"mediaItems":[{"uri":"spotify:playlist:37i9dQZF1E8abc"}]}"#;
        assert_eq!(
            radio_playlist(answer).as_deref(),
            Some("spotify:playlist:37i9dQZF1E8abc")
        );
        assert_eq!(radio_playlist(br#"{"mediaItems":[]}"#), None);
        assert_eq!(radio_playlist(b"not json"), None);
    }
}
