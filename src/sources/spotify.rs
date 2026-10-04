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
    items: Vec<T>,
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

/// Spotify desktop player client ID (spotify-player / spotatui standard, supports streaming + Web API).
pub const DEFAULT_SPOTIFY_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";

/// Redirect URI for the default player client ID.
#[cfg(feature = "spotify")]
pub const DEFAULT_SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:8989/login";

/// Return the appropriate redirect URI for a given Spotify client ID.
#[cfg(feature = "spotify")]
pub fn redirect_uri_for_client(client_id: &str) -> &'static str {
    if client_id == DEFAULT_SPOTIFY_CLIENT_ID {
        DEFAULT_SPOTIFY_REDIRECT_URI
    } else {
        "http://127.0.0.1:8898/login"
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

    pub fn user_name(&self) -> Option<&str> {
        self.user_name.as_deref()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn client_id(&self) -> Option<&str> {
        self.client_id.as_deref()
    }

    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub async fn verify_or_refresh_session(&mut self) -> Result<(), String> {
        if let Some(ref token) = self.access_token {
            let resp = self
                .http
                .get("https://api.spotify.com/v1/me")
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            if resp.status().is_success() {
                if let Ok(me) = resp.json::<SpotifyUserMe>().await {
                    let is_prem = me.product.as_deref() == Some("premium");
                    self.is_premium = is_prem;
                    self.user_name = me.display_name.or(Some(me.id));
                    if !is_prem {
                        return Err("Spotify Premium is required.".into());
                    }
                    return Ok(());
                }
            }
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

        let redirect_uri = redirect_uri_for_client(&client_id);
        let oauth_client =
            librespot_oauth::OAuthClientBuilder::new(&client_id, redirect_uri, vec![])
                .build()
                .map_err(|e| e.to_string())?;

        let new_token = oauth_client
            .refresh_token_async(&refresh_tok)
            .await
            .map_err(|e| format!("Spotify token refresh failed: {}", e))?;

        self.access_token = Some(new_token.access_token.clone());
        self.refresh_token = Some(new_token.refresh_token);
        self.token_issued_at = Some(std::time::Instant::now());

        // Persist immediately: the old refresh token may no longer be valid.
        let mut creds = crate::config::credentials::Credentials::load();
        creds.spotify_client_id = Some(client_id);
        let cache = serde_json::json!({
            "access_token": self.access_token.clone(),
            "refresh_token": self.refresh_token.clone(),
        });
        creds.spotify_token_cache = Some(cache.to_string());
        let _ = creds.save();

        let resp = self
            .http
            .get("https://api.spotify.com/v1/me")
            .bearer_auth(&new_token.access_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!(
                "Failed to fetch user profile: HTTP {}",
                resp.status()
            ));
        }
        let me = resp
            .json::<SpotifyUserMe>()
            .await
            .map_err(|e| format!("Failed to parse user profile: {}", e))?;
        let is_prem = me.product.as_deref() == Some("premium");
        self.is_premium = is_prem;
        self.user_name = me.display_name.or(Some(me.id));
        if !is_prem {
            return Err("Spotify Premium is required.".into());
        }
        Ok(())
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
            let _ = self.refresh_session().await;
        }
    }

    pub async fn login(&mut self, input_client_id: &str) -> Result<String, String> {
        let client_id = if input_client_id.trim().is_empty() || input_client_id.trim() == "default"
        {
            DEFAULT_SPOTIFY_CLIENT_ID
        } else {
            input_client_id.trim()
        };
        let redirect_uri = redirect_uri_for_client(client_id);

        let scopes = vec![
            "user-read-playback-state",
            "user-modify-playback-state",
            "user-read-currently-playing",
            "streaming",
            "playlist-read-private",
            "playlist-read-collaborative",
            "user-library-read",
            "user-read-private",
            "user-read-recently-played",
            "user-top-read",
        ];

        let oauth_client =
            librespot_oauth::OAuthClientBuilder::new(client_id, redirect_uri, scopes)
                .open_in_browser()
                .build()
                .map_err(|e| format!("Failed to create OAuth client: {}", e))?;

        let token = oauth_client
            .get_access_token_async()
            .await
            .map_err(|e| format!("OAuth login failed: {}", e))?;

        self.client_id = Some(client_id.to_string());
        self.access_token = Some(token.access_token.clone());
        self.refresh_token = Some(token.refresh_token.clone());
        self.token_issued_at = Some(std::time::Instant::now());

        // Check user profile and Premium status
        let resp = self
            .http
            .get("https://api.spotify.com/v1/me")
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!(
                "Failed to fetch user profile: HTTP {}",
                resp.status()
            ));
        }

        let me = resp
            .json::<SpotifyUserMe>()
            .await
            .map_err(|e| format!("Failed to parse user profile: {}", e))?;

        let is_prem = me.product.as_deref() == Some("premium");
        self.is_premium = is_prem;
        let display_name = me.display_name.unwrap_or(me.id);
        self.user_name = Some(display_name.clone());

        if !is_prem {
            return Err("Spotify Premium is required.".into());
        }

        Ok(format!("Connected to Spotify as {}", display_name))
    }

    pub async fn search(&self, query: &str, page: usize) -> Result<Vec<BrowseItem>, String> {
        let Some(ref token) = self.access_token else {
            return Err("Spotify access token missing".into());
        };

        let offset_str = (page * SEARCH_PAGE_SIZE).to_string();
        let mut resp = self
            .http
            .get("https://api.spotify.com/v1/search")
            .query(&[
                ("q", query),
                ("type", "track,album,playlist"),
                ("limit", SEARCH_PAGE_SIZE_STR),
                ("offset", &offset_str),
            ])
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if resp.status().as_u16() == 429 {
            let retry_secs = resp
                .headers()
                .get("Retry-After")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(2)
                .min(5);
            tokio::time::sleep(std::time::Duration::from_secs(retry_secs)).await;
            resp = self
                .http
                .get("https://api.spotify.com/v1/search")
                .query(&[
                    ("q", query),
                    ("type", "track,album,playlist"),
                    ("limit", SEARCH_PAGE_SIZE_STR),
                    ("offset", &offset_str),
                ])
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| e.to_string())?;
        }

        if !resp.status().is_success() {
            return Err(format!("Search failed: HTTP {}", resp.status()));
        }

        let search_data = resp
            .json::<SpotifySearchResponse>()
            .await
            .map_err(|e| e.to_string())?;

        let mut items = Vec::new();

        // 1. Tracks
        if let Some(tracks) = search_data.tracks {
            for track in tracks.items {
                let artists: Vec<String> = track.artists.into_iter().map(|a| a.name).collect();
                let cover = track
                    .album
                    .as_ref()
                    .and_then(|alb| best_image(alb.images.as_deref()));
                items.push(BrowseItem {
                    id: format!("spotify:track:{}", track.id),
                    title: track.name,
                    subtitle: Some(artists.join(", ")),
                    kind: BrowseItemKind::Track,
                    is_container: false,
                    track_ref: Some(TrackRef::Spotify(track.uri)),
                    duration_secs: Some(track.duration_ms / 1000),
                    artwork_url: cover,
                    depth: 0,
                });
            }
        }

        // 2. Albums
        if let Some(albums) = search_data.albums {
            for album in albums.items {
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
                    depth: 0,
                });
            }
        }

        // 3. Playlists
        if let Some(playlists) = search_data.playlists {
            for pl in playlists.items {
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
                depth: 0,
            }]
        }
    }

    async fn fetch_page_with_retry(
        &self,
        url: &str,
        token: &str,
    ) -> Result<reqwest::Response, String> {
        let mut retries = 0;
        loop {
            let resp = self
                .http
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                retries += 1;
                if retries > 5 {
                    return Err("Spotify API rate limit exceeded (HTTP 429)".into());
                }
                let retry_secs = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1);
                tokio::time::sleep(std::time::Duration::from_secs(retry_secs.clamp(1, 5))).await;
                continue;
            }

            return Ok(resp);
        }
    }

    /// Stream container children page by page via event channel so first paint is instantaneous (<300ms).
    pub async fn stream_children(
        &self,
        parent_id: &str,
        event_tx: crossbeam_channel::Sender<SourceEvent>,
        source: SourceTab,
    ) -> Result<(), String> {
        let Some(ref token) = self.access_token else {
            return Err("Spotify access token missing".into());
        };

        if parent_id == "spotify:recently_played" {
            let url = "https://api.spotify.com/v1/me/player/recently-played?limit=50";
            let resp = self.fetch_page_with_retry(url, token).await?;
            if resp.status() == reqwest::StatusCode::FORBIDDEN {
                return Err("Missing permissions. Sign out and log in again to grant recently played access.".into());
            }
            if !resp.status().is_success() {
                return Err(format!(
                    "Failed to fetch recently played: HTTP {}",
                    resp.status()
                ));
            }
            let data = resp
                .json::<SpotifyRecentlyPlayedResponse>()
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<_> = data
                .items
                .into_iter()
                .map(|item| {
                    let track = item.track;
                    let artists: Vec<String> = track.artists.into_iter().map(|a| a.name).collect();
                    let cover = track
                        .album
                        .as_ref()
                        .and_then(|alb| best_image(alb.images.as_deref()));
                    BrowseItem {
                        id: format!("spotify:track:{}", track.id),
                        title: track.name,
                        subtitle: Some(artists.join(", ")),
                        kind: BrowseItemKind::Track,
                        is_container: false,
                        track_ref: Some(TrackRef::Spotify(track.uri)),
                        duration_secs: Some(track.duration_ms / 1000),
                        artwork_url: cover,
                        depth: 0,
                    }
                })
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
            let resp = self.fetch_page_with_retry(url, token).await?;
            if resp.status() == reqwest::StatusCode::FORBIDDEN {
                return Err(
                    "Missing permissions. Sign out and log in again to grant top tracks access."
                        .into(),
                );
            }
            if !resp.status().is_success() {
                return Err(format!(
                    "Failed to fetch top tracks: HTTP {}",
                    resp.status()
                ));
            }
            let data = resp
                .json::<SpotifyPaging<SpotifyTrack>>()
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<_> = data
                .items
                .into_iter()
                .map(|track| {
                    let artists: Vec<String> = track.artists.into_iter().map(|a| a.name).collect();
                    let cover = track
                        .album
                        .as_ref()
                        .and_then(|alb| best_image(alb.images.as_deref()));
                    BrowseItem {
                        id: format!("spotify:track:{}", track.id),
                        title: track.name,
                        subtitle: Some(artists.join(", ")),
                        kind: BrowseItemKind::Track,
                        is_container: false,
                        track_ref: Some(TrackRef::Spotify(track.uri)),
                        duration_secs: Some(track.duration_ms / 1000),
                        artwork_url: cover,
                        depth: 0,
                    }
                })
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
                let resp = self.fetch_page_with_retry(&url, token).await?;
                if !resp.status().is_success() {
                    return Err(format!(
                        "Failed to fetch liked songs: HTTP {}",
                        resp.status()
                    ));
                }

                let data = resp
                    .json::<SpotifyPaging<SpotifySavedTrackItem>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .map(|item| {
                        let track = item.track;
                        let artists: Vec<String> =
                            track.artists.into_iter().map(|a| a.name).collect();
                        let cover = track
                            .album
                            .as_ref()
                            .and_then(|alb| best_image(alb.images.as_deref()));
                        BrowseItem {
                            id: format!("spotify:track:{}", track.id),
                            title: track.name,
                            subtitle: Some(artists.join(", ")),
                            kind: BrowseItemKind::Track,
                            is_container: false,
                            track_ref: Some(TrackRef::Spotify(track.uri)),
                            duration_secs: Some(track.duration_ms / 1000),
                            artwork_url: cover,
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

        if parent_id == "spotify:library_playlists" {
            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/me/playlists?limit=50&offset={}",
                    current_offset
                );
                let resp = self.fetch_page_with_retry(&url, token).await?;
                if !resp.status().is_success() {
                    return Err(format!("Failed to fetch playlists: HTTP {}", resp.status()));
                }

                let data = resp
                    .json::<SpotifyPaging<SpotifyPlaylistSimple>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
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
                let resp = self.fetch_page_with_retry(&url, token).await?;
                if !resp.status().is_success() {
                    return Err(format!("Failed to fetch albums: HTTP {}", resp.status()));
                }

                let data = resp
                    .json::<SpotifyPaging<SpotifySavedAlbumItem>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
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
                    "https://api.spotify.com/v1/playlists/{}/items?limit=100&offset={}",
                    pid, current_offset
                );
                let resp = self.fetch_page_with_retry(&url, token).await?;
                if !resp.status().is_success() {
                    return Err(format!(
                        "Failed to fetch playlist tracks: HTTP {}",
                        resp.status()
                    ));
                }

                let data = resp
                    .json::<SpotifyPaging<SpotifyPlaylistTrackItem>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .filter_map(SpotifyPlaylistTrackItem::into_track)
                    .map(|track| {
                        let artists: Vec<String> =
                            track.artists.into_iter().map(|a| a.name).collect();
                        let cover = track
                            .album
                            .as_ref()
                            .and_then(|alb| best_image(alb.images.as_deref()));
                        BrowseItem {
                            id: format!("spotify:track:{}", track.id),
                            title: track.name,
                            subtitle: Some(artists.join(", ")),
                            kind: BrowseItemKind::Track,
                            is_container: false,
                            track_ref: Some(TrackRef::Spotify(track.uri)),
                            duration_secs: Some(track.duration_ms / 1000),
                            artwork_url: cover,
                            depth: 0,
                        }
                    })
                    .collect();

                let is_last = raw_len < 100 || total.is_some_and(|t| current_offset + raw_len >= t);
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
            let mut current_offset = 0;
            let mut page_idx = 0;
            loop {
                let url = format!(
                    "https://api.spotify.com/v1/albums/{}/tracks?limit=50&offset={}",
                    aid, current_offset
                );
                let resp = self.fetch_page_with_retry(&url, token).await?;
                if !resp.status().is_success() {
                    return Err(format!(
                        "Failed to fetch album tracks: HTTP {}",
                        resp.status()
                    ));
                }

                let data = resp
                    .json::<SpotifyPaging<SpotifyTrack>>()
                    .await
                    .map_err(|e| e.to_string())?;

                let raw_len = data.items.len();
                let total = data.total;

                let page_items: Vec<_> = data
                    .items
                    .into_iter()
                    .map(|track| {
                        let artists: Vec<String> =
                            track.artists.into_iter().map(|a| a.name).collect();
                        BrowseItem {
                            id: format!("spotify:track:{}", track.id),
                            title: track.name,
                            subtitle: Some(artists.join(", ")),
                            kind: BrowseItemKind::Track,
                            is_container: false,
                            track_ref: Some(TrackRef::Spotify(track.uri)),
                            duration_secs: Some(track.duration_ms / 1000),
                            artwork_url: None,
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
    pub fn client_id(&self) -> Option<&str> {
        None
    }
    pub fn last_error(&self) -> Option<&str> {
        None
    }
    pub fn access_token(&self) -> Option<&str> {
        None
    }
    pub fn refresh_token(&self) -> Option<&str> {
        None
    }
    pub async fn ensure_fresh_token(&mut self) {}
    pub async fn login(&mut self, _client_id: &str) -> Result<String, String> {
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
        assert_eq!(
            DEFAULT_SPOTIFY_CLIENT_ID,
            "65b708073fc0480ea92a077233ca87bd"
        );
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
