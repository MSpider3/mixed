use super::BrowseItem;
#[cfg(feature = "spotify")]
use super::{BrowseItemKind, SourceEvent, SourceTab};
#[cfg(feature = "spotify")]
use crate::data::track::TrackRef;

#[cfg(feature = "spotify")]
use serde::Deserialize;

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyImage {
    url: String,
    width: Option<u32>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyArtistSimple {
    name: String,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyAlbumSimple {
    id: String,
    name: String,
    release_date: Option<String>,
    images: Option<Vec<SpotifyImage>>,
    artists: Option<Vec<SpotifyArtistSimple>>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyTrack {
    id: String,
    name: String,
    uri: String,
    duration_ms: u64,
    artists: Vec<SpotifyArtistSimple>,
    album: Option<SpotifyAlbumSimple>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyPlaylistSimple {
    id: String,
    name: String,
    images: Option<Vec<SpotifyImage>>,
    owner: Option<SpotifyOwner>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyOwner {
    display_name: Option<String>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifySearchResponse {
    tracks: Option<SpotifyPaging<SpotifyTrack>>,
    albums: Option<SpotifyPaging<SpotifyAlbumSimple>>,
    playlists: Option<SpotifyPaging<SpotifyPlaylistSimple>>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyPaging<T> {
    /// Spotify fills in `null` for entries it will not show (e.g. in playlist searches).
    items: Vec<Option<T>>,
    #[allow(dead_code)]
    total: Option<usize>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifySavedTrackItem {
    track: SpotifyTrack,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifySavedAlbumItem {
    album: SpotifyAlbumSimple,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyRecentlyPlayedResponse {
    items: Vec<SpotifyRecentlyPlayedItem>,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyRecentlyPlayedItem {
    track: SpotifyTrack,
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyPlaylistTrackItem {
    /// Current field name; holds a track or an episode object.
    item: Option<serde_json::Value>,
    /// Deprecated predecessor of `item`, still sent alongside it.
    track: Option<serde_json::Value>,
}

#[cfg(feature = "spotify")]
impl SpotifyPlaylistTrackItem {
    /// The entry as a track, or `None` for episodes, local files and removed tracks.
    fn into_track(self) -> Option<SpotifyTrack> {
        self.item
            .or(self.track)
            .and_then(|v| serde_json::from_value(v).ok())
    }
}

#[cfg(feature = "spotify")]
#[derive(Debug, Deserialize)]
struct SpotifyUserMe {
    id: String,
    display_name: Option<String>,
    product: Option<String>,
}

#[cfg(feature = "spotify")]
fn best_image(images: Option<&[SpotifyImage]>) -> Option<String> {
    images?
        .iter()
        .max_by_key(|img| img.width.unwrap_or(0))
        .map(|img| img.url.clone())
}

/// A track as a browse item. `album` is the album of tracks that are listed without
/// one (the tracks of an album).
#[cfg(feature = "spotify")]
fn track_item(track: SpotifyTrack, album: Option<&SpotifyAlbumSimple>) -> BrowseItem {
    let album = track.album.as_ref().or(album);
    let artists: Vec<String> = track.artists.into_iter().map(|a| a.name).collect();
    BrowseItem {
        id: format!("spotify:track:{}", track.id),
        title: track.name,
        subtitle: Some(artists.join(", ")),
        kind: BrowseItemKind::Track,
        is_container: false,
        track_ref: Some(TrackRef::Spotify(track.uri)),
        duration_secs: Some(track.duration_ms / 1000),
        artwork_url: album.and_then(|a| best_image(a.images.as_deref())),
        album: album.map(|a| a.name.clone()),
        depth: 0,
    }
}

#[cfg(feature = "spotify")]
pub struct SpotifyClient {
    client_id: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    user_name: Option<String>,
    is_premium: bool,
    /// When the access token was last issued; `None` means its age is unknown.
    token_issued_at: Option<std::time::Instant>,
    last_error: Option<String>,
    http: reqwest::Client,
}

/// The Search endpoint accepts `limit` in the range 0-10 (per result type).
#[cfg(feature = "spotify")]
pub const SEARCH_PAGE_SIZE: usize = 10;
#[cfg(feature = "spotify")]
const SEARCH_PAGE_SIZE_STR: &str = "10";

/// Spotify access tokens live for one hour; refresh comfortably before that.
#[cfg(feature = "spotify")]
const TOKEN_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(45 * 60);

/// A rate limit longer than this is reported instead of waited out.
#[cfg(feature = "spotify")]
const MAX_RATE_LIMIT_WAIT_SECS: u64 = 30;
#[cfg(feature = "spotify")]
const MAX_RATE_LIMIT_RETRIES: u32 = 5;

/// Default client ID for the Web API (search and library): the one ncspot,
/// spotify-player and spotatui share. Spotify rate-limits the streaming client ID
/// below on the Web API, so the two jobs need separate sign-ins.
pub const DEFAULT_SPOTIFY_CLIENT_ID: &str = "d420a117a32841c2b3474932e49fb54b";

/// Client ID of Spotify's desktop player, which librespot signs in with for audio.
pub const SPOTIFY_STREAMING_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";

/// Redirect URI registered for both built-in client IDs.
#[cfg(feature = "spotify")]
pub const DEFAULT_SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8989/login";

#[cfg(feature = "spotify")]
const WEB_API_SCOPES: &[&str] = &[
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-library-read",
    "user-read-private",
    "user-read-recently-played",
    "user-top-read",
];

/// Return the appropriate redirect URI for a given Spotify client ID.
#[cfg(feature = "spotify")]
pub fn redirect_uri_for_client(client_id: &str) -> &'static str {
    if client_id == DEFAULT_SPOTIFY_CLIENT_ID {
        DEFAULT_SPOTIFY_REDIRECT_URI
    } else {
        "http://127.0.0.1:8898/login"
    }
}

/// Describe a failed Web API response, with Spotify's own message when it sent one.
#[cfg(feature = "spotify")]
fn describe_api_error(status: reqwest::StatusCode, body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_string));
    match message {
        Some(message) => format!("Spotify API error: HTTP {} ({})", status, message),
        None => format!("Spotify API error: HTTP {}", status),
    }
}

#[cfg(feature = "spotify")]
impl SpotifyClient {
    pub async fn new(
        client_id: Option<String>,
        access_token: Option<String>,
        refresh_token: Option<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        let mut client = Self {
            client_id,
            access_token,
            refresh_token,
            user_name: None,
            is_premium: false,
            token_issued_at: None,
            last_error: None,
            http,
        };

        if client.access_token.is_some() || client.refresh_token.is_some() {
            if let Err(e) = client.verify_or_refresh_session().await {
                log::warn!("Spotify startup session verification failed: {}", e);
                client.last_error = Some(e);
            }
        }

        client
    }

    pub fn is_authenticated(&self) -> bool {
        self.access_token.is_some() && self.is_premium
    }

    /// True once playback has been signed in as well (the second step of `login`).
    pub fn can_stream(&self) -> bool {
        crate::audio::spotify_backend::has_stored_credentials()
    }

    pub fn user_name(&self) -> Option<&str> {
        self.user_name.as_deref()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// GET a Web API URL with the current access token. Short rate limits are waited
    /// out; any other failure is returned with Spotify's explanation.
    async fn get(&self, url: &str) -> Result<reqwest::Response, String> {
        let Some(token) = self.access_token.as_deref() else {
            return Err("Spotify access token missing".into());
        };

        let mut retries = 0;
        loop {
            let resp = self
                .http
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_secs = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1)
                    .max(1);
                retries += 1;
                // Asking again before the window ends only prolongs the limit.
                if retries <= MAX_RATE_LIMIT_RETRIES && retry_secs <= MAX_RATE_LIMIT_WAIT_SECS {
                    tokio::time::sleep(std::time::Duration::from_secs(retry_secs)).await;
                    continue;
                }
                log::warn!(
                    "Spotify rate limit on {}: retry after {} s",
                    url,
                    retry_secs
                );
                return Err(format!(
                    "Spotify is rate limiting requests. Try again in {} s.",
                    retry_secs
                ));
            }

            let body = resp.text().await.unwrap_or_default();
            log::warn!("Spotify API returned {} for {}: {}", status, url, body);
            return Err(describe_api_error(status, &body));
        }
    }

    /// Fetch the user's name and subscription with the current access token.
    async fn load_profile(&mut self) -> Result<(), String> {
        let me = self
            .get("https://api.spotify.com/v1/me")
            .await?
            .json::<SpotifyUserMe>()
            .await
            .map_err(|e| format!("Failed to parse user profile: {}", e))?;
        self.is_premium = me.product.as_deref() == Some("premium");
        self.user_name = me.display_name.or(Some(me.id));
        if !self.is_premium {
            return Err("Spotify Premium is required.".into());
        }
        Ok(())
    }

    /// Take over newly issued tokens and persist them: an old refresh token may
    /// no longer be valid once a new one exists.
    fn adopt_token(&mut self, token: super::spotify_oauth::Token) {
        self.access_token = Some(token.access_token);
        // A refresh without a new refresh token leaves the current one valid.
        if token.refresh_token.is_some() {
            self.refresh_token = token.refresh_token;
        }
        self.token_issued_at = Some(std::time::Instant::now());

        let mut creds = crate::config::credentials::Credentials::load();
        creds.spotify_client_id = self.client_id.clone();
        let cache = serde_json::json!({
            "access_token": self.access_token,
            "refresh_token": self.refresh_token,
        });
        creds.spotify_token_cache = Some(cache.to_string());
        if let Err(e) = creds.save() {
            log::warn!("Could not save Spotify credentials: {}", e);
        }
    }

    pub async fn verify_or_refresh_session(&mut self) -> Result<(), String> {
        if self.access_token.is_some() && self.load_profile().await.is_ok() {
            return Ok(());
        }
        self.refresh_session().await
    }

    /// Exchange the refresh token for a new access token and persist both.
    async fn refresh_session(&mut self) -> Result<(), String> {
        let (Some(client_id), Some(refresh_tok)) =
            (self.client_id.clone(), self.refresh_token.clone())
        else {
            return Err("No valid Spotify session".into());
        };

        let token = super::spotify_oauth::refresh(&client_id, &refresh_tok).await?;
        self.adopt_token(token);
        self.load_profile().await
    }

    /// Refresh the access token when it is old (or of unknown age) so long
    /// sessions do not start failing with HTTP 401 after an hour.
    pub async fn ensure_fresh_token(&mut self) {
        if self.refresh_token.is_none() || self.client_id.is_none() {
            return;
        }
        let stale = self
            .token_issued_at
            .is_none_or(|at| at.elapsed() >= TOKEN_MAX_AGE);
        if stale {
            // On failure keep the current token; the request reports its own error.
            if let Err(e) = self.refresh_session().await {
                log::warn!("{}", e);
            }
        }
    }

    /// Sign in through the browser. Two consents are needed, one for the Web API and
    /// one for playback, since they use different client IDs. A consent that is
    /// already in place is skipped, so a failed sign-in resumes where it stopped.
    /// `progress` is told what the user has to do next.
    pub async fn login(
        &mut self,
        input_client_id: &str,
        progress: impl Fn(&str),
    ) -> Result<String, String> {
        let client_id = match input_client_id.trim() {
            "" => self
                .client_id
                .clone()
                .unwrap_or_else(|| DEFAULT_SPOTIFY_CLIENT_ID.to_string()),
            "default" => DEFAULT_SPOTIFY_CLIENT_ID.to_string(),
            id => id.to_string(),
        };
        // Spotify would only say so in the browser, leaving the sign-in to time out.
        if client_id.len() != 32 || !client_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("A Spotify Client ID is 32 letters and digits.".into());
        }

        if self.client_id.as_deref() != Some(client_id.as_str())
            || self.verify_or_refresh_session().await.is_err()
        {
            progress("Step 1 of 2: approve library access in your browser...");
            let token = super::spotify_oauth::authorize(
                &client_id,
                redirect_uri_for_client(&client_id),
                WEB_API_SCOPES,
            )
            .await?;
            self.client_id = Some(client_id);
            self.adopt_token(token);
            self.load_profile().await?;
        }

        if !self.can_stream() {
            progress("Step 2 of 2: approve playback in your browser...");
            let token = super::spotify_oauth::authorize(
                SPOTIFY_STREAMING_CLIENT_ID,
                DEFAULT_SPOTIFY_REDIRECT_URI,
                &["streaming"],
            )
            .await?;
            crate::audio::spotify_backend::store_credentials(&token.access_token).await?;
        }

        self.last_error = None;
        Ok(format!(
            "Connected to Spotify as {}",
            self.user_name.as_deref().unwrap_or("unknown")
        ))
    }

    pub async fn search(&self, query: &str, page: usize) -> Result<Vec<BrowseItem>, String> {
        let offset_str = (page * SEARCH_PAGE_SIZE).to_string();
        let url = reqwest::Url::parse_with_params(
            "https://api.spotify.com/v1/search",
            &[
                ("q", query),
                ("type", "track,album,playlist"),
                ("limit", SEARCH_PAGE_SIZE_STR),
                ("offset", &offset_str),
            ],
        )
        .map_err(|e| e.to_string())?;
        let resp = self.get(url.as_str()).await?;

        let search_data = resp
            .json::<SpotifySearchResponse>()
            .await
            .map_err(|e| e.to_string())?;

        let mut items = Vec::new();

        // 1. Tracks
        if let Some(tracks) = search_data.tracks {
            items.extend(
                tracks
                    .items
                    .into_iter()
                    .flatten()
                    .map(|track| track_item(track, None)),
            );
        }

        // 2. Albums
        if let Some(albums) = search_data.albums {
            for album in albums.items.into_iter().flatten() {
                let artists: Vec<String> = album
                    .artists
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| a.name)
                    .collect();
                let subtitle = if let Some(year) = album.release_date {
                    Some(format!("{} • {}", artists.join(", "), year))
                } else {
                    Some(artists.join(", "))
                };
                items.push(BrowseItem {
                    id: format!("spotify:album:{}", album.id),
                    title: album.name,
                    subtitle,
                    kind: BrowseItemKind::Album,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: best_image(album.images.as_deref()),
                    album: None,
                    depth: 0,
                });
            }
        }

        // 3. Playlists
        if let Some(playlists) = search_data.playlists {
            for pl in playlists.items.into_iter().flatten() {
                let owner = pl
                    .owner
                    .and_then(|o| o.display_name)
                    .unwrap_or_else(|| "Spotify".into());
                items.push(BrowseItem {
                    id: format!("spotify:playlist:{}", pl.id),
                    title: pl.name,
                    subtitle: Some(format!("Playlist • {}", owner)),
                    kind: BrowseItemKind::Playlist,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: best_image(pl.images.as_deref()),
                    album: None,
                    depth: 0,
                });
            }
        }

        Ok(items)
    }

    pub fn library_roots(&self) -> Vec<BrowseItem> {
        if self.is_authenticated() {
            vec![
                BrowseItem {
                    id: "spotify:liked_songs".into(),
                    title: "Liked Songs".into(),
                    subtitle: Some("Your library".into()),
                    kind: BrowseItemKind::Playlist,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: None,
                    album: None,
                    depth: 0,
                },
                BrowseItem {
                    id: "spotify:recently_played".into(),
                    title: "Recently Played".into(),
                    subtitle: Some("Your history".into()),
                    kind: BrowseItemKind::Playlist,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: None,
                    album: None,
                    depth: 0,
                },
                BrowseItem {
                    id: "spotify:top_tracks".into(),
                    title: "Top Tracks".into(),
                    subtitle: Some("Your favorites".into()),
                    kind: BrowseItemKind::Playlist,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: None,
                    album: None,
                    depth: 0,
                },
                BrowseItem {
                    id: "spotify:library_playlists".into(),
                    title: "Playlists".into(),
                    subtitle: Some("Your library".into()),
                    kind: BrowseItemKind::Playlist,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: None,
                    album: None,
                    depth: 0,
                },
                BrowseItem {
                    id: "spotify:library_albums".into(),
                    title: "Albums".into(),
                    subtitle: Some("Your library".into()),
                    kind: BrowseItemKind::Album,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: None,
                    album: None,
                    depth: 0,
                },
            ]
        } else {
            vec![BrowseItem {
                id: "spotify:login_hint".into(),
                title: "Login with Spotify Client ID".into(),
                subtitle: Some("Set spotify_client_id or paste your client ID".into()),
                kind: BrowseItemKind::Playlist,
                is_container: false,
                track_ref: None,
                duration_secs: None,
                artwork_url: None,
                album: None,
                depth: 0,
            }]
        }
    }

    /// Stream container children page by page via event channel so first paint is instantaneous (<300ms).
    pub async fn stream_children(
        &self,
        parent_id: &str,
        event_tx: crossbeam_channel::Sender<SourceEvent>,
        source: SourceTab,
    ) -> Result<(), String> {
        if parent_id == "spotify:recently_played" {
            let url = "https://api.spotify.com/v1/me/player/recently-played?limit=50";
            let resp = self.get(url).await?;
            let data = resp
                .json::<SpotifyRecentlyPlayedResponse>()
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<_> = data
                .items
                .into_iter()
                .map(|item| track_item(item.track, None))
                .collect();
            let _ = event_tx.send(SourceEvent::Children {
                source,
                parent_id: parent_id.to_string(),
                items,
                page: 0,
                has_more: false,
            });
            return Ok(());
        }

        if parent_id == "spotify:top_tracks" {
            let url = "https://api.spotify.com/v1/me/top/tracks?limit=50";
            let resp = self.get(url).await?;
            let data = resp
                .json::<SpotifyPaging<SpotifyTrack>>()
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<_> = data
                .items
                .into_iter()
                .flatten()
                .map(|track| track_item(track, None))
                .collect();
            let _ = event_tx.send(SourceEvent::Children {
                source,
                parent_id: parent_id.to_string(),
                items,
                page: 0,
                has_more: false,
            });
            return Ok(());
        }

        if parent_id == "spotify:liked_songs" {
            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/me/tracks?limit=50&offset={}",
                    current_offset
                );
                let resp = self.get(&url).await?;

                let data = resp
                    .json::<SpotifyPaging<SpotifySavedTrackItem>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .flatten()
                    .map(|item| track_item(item.track, None))
                    .collect();

                let is_last = raw_len < 50 || total.is_some_and(|t| current_offset + raw_len >= t);
                let _ = event_tx.send(SourceEvent::Children {
                    source,
                    parent_id: parent_id.to_string(),
                    items: page_items,
                    page: page_idx,
                    has_more: !is_last,
                });

                if is_last {
                    break;
                }
                current_offset += raw_len;
                page_idx += 1;
            }
            return Ok(());
        }

        if parent_id == "spotify:library_playlists" {
            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/me/playlists?limit=50&offset={}",
                    current_offset
                );
                let resp = self.get(&url).await?;

                let data = resp
                    .json::<SpotifyPaging<SpotifyPlaylistSimple>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .flatten()
                    .map(|pl| {
                        let owner = pl
                            .owner
                            .and_then(|o| o.display_name)
                            .unwrap_or_else(|| "Spotify".into());
                        BrowseItem {
                            id: format!("spotify:playlist:{}", pl.id),
                            title: pl.name,
                            subtitle: Some(format!("Playlist • {}", owner)),
                            kind: BrowseItemKind::Playlist,
                            is_container: true,
                            track_ref: None,
                            duration_secs: None,
                            artwork_url: best_image(pl.images.as_deref()),
                            album: None,
                            depth: 0,
                        }
                    })
                    .collect();

                let is_last = raw_len < 50 || total.is_some_and(|t| current_offset + raw_len >= t);
                let _ = event_tx.send(SourceEvent::Children {
                    source,
                    parent_id: parent_id.to_string(),
                    items: page_items,
                    page: page_idx,
                    has_more: !is_last,
                });

                if is_last {
                    break;
                }
                current_offset += raw_len;
                page_idx += 1;
            }
            return Ok(());
        }

        if parent_id == "spotify:library_albums" {
            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/me/albums?limit=50&offset={}",
                    current_offset
                );
                let resp = self.get(&url).await?;

                let data = resp
                    .json::<SpotifyPaging<SpotifySavedAlbumItem>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .flatten()
                    .map(|item| {
                        let album = item.album;
                        let artists: Vec<String> = album
                            .artists
                            .unwrap_or_default()
                            .into_iter()
                            .map(|a| a.name)
                            .collect();
                        BrowseItem {
                            id: format!("spotify:album:{}", album.id),
                            title: album.name,
                            subtitle: Some(artists.join(", ")),
                            kind: BrowseItemKind::Album,
                            is_container: true,
                            track_ref: None,
                            duration_secs: None,
                            artwork_url: best_image(album.images.as_deref()),
                            album: None,
                            depth: 0,
                        }
                    })
                    .collect();

                let is_last = raw_len < 50 || total.is_some_and(|t| current_offset + raw_len >= t);
                let _ = event_tx.send(SourceEvent::Children {
                    source,
                    parent_id: parent_id.to_string(),
                    items: page_items,
                    page: page_idx,
                    has_more: !is_last,
                });

                if is_last {
                    break;
                }
                current_offset += raw_len;
                page_idx += 1;
            }
            return Ok(());
        }

        if let Some(pid) = parent_id.strip_prefix("spotify:playlist:") {
            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/playlists/{}/items?limit=50&offset={}",
                    pid, current_offset
                );
                let resp = self.get(&url).await?;

                let data = resp
                    .json::<SpotifyPaging<SpotifyPlaylistTrackItem>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .flatten()
                    .filter_map(SpotifyPlaylistTrackItem::into_track)
                    .map(|track| track_item(track, None))
                    .collect();

                let is_last = raw_len < 50 || total.is_some_and(|t| current_offset + raw_len >= t);
                let _ = event_tx.send(SourceEvent::Children {
                    source,
                    parent_id: parent_id.to_string(),
                    items: page_items,
                    page: page_idx,
                    has_more: !is_last,
                });

                if is_last {
                    break;
                }
                current_offset += raw_len;
                page_idx += 1;
            }
            return Ok(());
        }

        if let Some(aid) = parent_id.strip_prefix("spotify:album:") {
            // An album's tracks are listed without the album: fetch it for its name and cover
            let album = self
                .get(&format!("https://api.spotify.com/v1/albums/{}", aid))
                .await?
                .json::<SpotifyAlbumSimple>()
                .await
                .map_err(|e| e.to_string())?;

            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/albums/{}/tracks?limit=50&offset={}",
                    aid, current_offset
                );
                let resp = self.get(&url).await?;

                let data = resp
                    .json::<SpotifyPaging<SpotifyTrack>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .flatten()
                    .map(|track| track_item(track, Some(&album)))
                    .collect();

                let is_last = raw_len < 50 || total.is_some_and(|t| current_offset + raw_len >= t);
                let _ = event_tx.send(SourceEvent::Children {
                    source,
                    parent_id: parent_id.to_string(),
                    items: page_items,
                    page: page_idx,
                    has_more: !is_last,
                });

                if is_last {
                    break;
                }
                current_offset += raw_len;
                page_idx += 1;
            }
            return Ok(());
        }

        Ok(())
    }

    pub async fn children(&self, parent_id: &str, _page: usize) -> Result<Vec<BrowseItem>, String> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.stream_children(parent_id, tx, SourceTab::Spotify)
            .await?;
        let mut all = Vec::new();
        while let Ok(SourceEvent::Children { items, .. }) = rx.try_recv() {
            all.extend(items);
        }
        Ok(all)
    }
}

#[cfg(not(feature = "spotify"))]
pub struct SpotifyClient;

#[cfg(not(feature = "spotify"))]
impl SpotifyClient {
    pub async fn new(
        _client_id: Option<String>,
        _access_token: Option<String>,
        _refresh_token: Option<String>,
    ) -> Self {
        Self
    }
    pub fn is_authenticated(&self) -> bool {
        false
    }
    pub fn user_name(&self) -> Option<&str> {
        None
    }
    pub fn last_error(&self) -> Option<&str> {
        None
    }
    pub async fn ensure_fresh_token(&mut self) {}
    pub fn can_stream(&self) -> bool {
        false
    }
    pub async fn login(
        &mut self,
        _client_id: &str,
        _progress: impl Fn(&str),
    ) -> Result<String, String> {
        Err("Spotify feature not enabled in build".into())
    }
    pub async fn search(&self, _query: &str, _page: usize) -> Result<Vec<BrowseItem>, String> {
        Err("Spotify feature not enabled in build".into())
    }
    pub fn library_roots(&self) -> Vec<BrowseItem> {
        Vec::new()
    }
    pub async fn stream_children(
        &self,
        _parent_id: &str,
        _event_tx: crossbeam_channel::Sender<super::SourceEvent>,
        _source: super::SourceTab,
    ) -> Result<(), String> {
        Err("Spotify feature not enabled in build".into())
    }
    pub async fn children(
        &self,
        _parent_id: &str,
        _page: usize,
    ) -> Result<Vec<BrowseItem>, String> {
        Err("Spotify feature not enabled in build".into())
    }
}

impl crate::sources::MusicSource for SpotifyClient {
    fn name(&self) -> &'static str {
        "Spotify"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spotify_client_id_and_redirect_uri() {
        // The Web API and playback must not share a client ID
        assert_ne!(DEFAULT_SPOTIFY_CLIENT_ID, SPOTIFY_STREAMING_CLIENT_ID);
        #[cfg(feature = "spotify")]
        {
            assert_eq!(
                redirect_uri_for_client(DEFAULT_SPOTIFY_CLIENT_ID),
                DEFAULT_SPOTIFY_REDIRECT_URI
            );
            assert_eq!(
                redirect_uri_for_client("custom-client-id"),
                "http://127.0.0.1:8898/login"
            );
        }
    }

    #[test]
    #[cfg(feature = "spotify")]
    fn paging_keeps_the_raw_count_and_skips_null_entries() {
        let page: SpotifyPaging<SpotifyPlaylistSimple> =
            serde_json::from_str(r#"{"items":[null,{"id":"p1","name":"Mix"}],"total":2}"#)
                .expect("a null entry does not fail the whole page");
        // Pagination advances by what Spotify sent, not by what is listed
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items.into_iter().flatten().count(), 1);
    }

    #[test]
    #[cfg(feature = "spotify")]
    fn api_errors_carry_spotifys_message() {
        let status = reqwest::StatusCode::FORBIDDEN;
        assert_eq!(
            describe_api_error(
                status,
                r#"{"error":{"status":403,"message":"Insufficient client scope"}}"#
            ),
            "Spotify API error: HTTP 403 Forbidden (Insufficient client scope)"
        );
        assert_eq!(
            describe_api_error(status, "<html>"),
            "Spotify API error: HTTP 403 Forbidden"
        );
    }

    #[test]
    #[cfg(feature = "spotify")]
    fn track_items_carry_album_and_cover() {
        let album_json = r#"{"id":"a1","name":"The Album","images":[{"url":"small","width":64},{"url":"big","width":640}]}"#;
        let track_json = |album: &str| {
            format!(
                r#"{{"id":"t1","name":"Song","uri":"spotify:track:t1","duration_ms":61000,"artists":[{{"name":"A"}},{{"name":"B"}}]{}}}"#,
                album
            )
        };

        let full: SpotifyTrack =
            serde_json::from_str(&track_json(&format!(r#","album":{}"#, album_json))).unwrap();
        let item = track_item(full, None);
        assert_eq!(item.subtitle.as_deref(), Some("A, B"));
        assert_eq!(item.album.as_deref(), Some("The Album"));
        assert_eq!(item.artwork_url.as_deref(), Some("big"));

        // The tracks of an album come without it and take it from the album itself
        let bare: SpotifyTrack = serde_json::from_str(&track_json("")).unwrap();
        let album: SpotifyAlbumSimple = serde_json::from_str(album_json).unwrap();
        let item = track_item(bare, Some(&album));
        assert_eq!(item.album.as_deref(), Some("The Album"));
        assert_eq!(item.artwork_url.as_deref(), Some("big"));
    }

    #[test]
    #[cfg(feature = "spotify")]
    fn test_library_roots_structure() {
        let client = SpotifyClient {
            client_id: Some("id".into()),
            access_token: Some("token".into()),
            refresh_token: Some("refresh".into()),
            user_name: Some("Test User".into()),
            is_premium: true,
            token_issued_at: None,
            last_error: None,
            http: reqwest::Client::new(),
        };
        let roots = client.library_roots();
        let root_ids: Vec<_> = roots.iter().map(|r| r.id.as_str()).collect();
        assert!(root_ids.contains(&"spotify:liked_songs"));
        assert!(root_ids.contains(&"spotify:recently_played"));
        assert!(root_ids.contains(&"spotify:top_tracks"));
        assert!(root_ids.contains(&"spotify:library_playlists"));
        assert!(root_ids.contains(&"spotify:library_albums"));
    }
}
