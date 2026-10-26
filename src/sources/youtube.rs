use super::BrowseItem;
#[cfg(feature = "youtube")]
use super::BrowseItemKind;
#[cfg(feature = "youtube")]
use crate::data::track::TrackRef;

/// Parse duration string formatted as "M:SS" or "H:MM:SS" into seconds.
pub fn parse_duration_str(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    match parts.len() {
        1 => parts[0].parse::<u64>().ok(),
        2 => {
            let mins = parts[0].parse::<u64>().ok()?;
            let secs = parts[1].parse::<u64>().ok()?;
            Some(mins * 60 + secs)
        }
        3 => {
            let hours = parts[0].parse::<u64>().ok()?;
            let mins = parts[1].parse::<u64>().ok()?;
            let secs = parts[2].parse::<u64>().ok()?;
            Some(hours * 3600 + mins * 60 + secs)
        }
        _ => None,
    }
}

/// YouTube Music lists tracks with a cover thumbnail of 120 pixels. The image is
/// served in whatever size its address names, so ask for the size albums are shown in.
pub fn full_size_cover(url: &str) -> String {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let resized = || {
        let start = url
            .rfind("=w")
            .filter(|_| url.contains("googleusercontent.com"))?;
        let width = &url[start + 2..];
        let height = width[digits(width)..].strip_prefix("-h")?;
        if digits(width) == 0 || digits(height) == 0 {
            return None;
        }
        Some(format!(
            "{}=w544-h544{}",
            &url[..start],
            &height[digits(height)..]
        ))
    };
    resized().unwrap_or_else(|| url.to_string())
}

#[cfg(feature = "youtube")]
use ytmapi_rs::auth::noauth::NoAuthToken;
#[cfg(feature = "youtube")]
use ytmapi_rs::auth::BrowserToken;
#[cfg(feature = "youtube")]
use ytmapi_rs::common::{AlbumID, MoodCategoryParams, PlaylistID, Thumbnail, VideoID, YoutubeID};
#[cfg(feature = "youtube")]
use ytmapi_rs::parse::WatchPlaylistTrack;
#[cfg(feature = "youtube")]
use ytmapi_rs::query::{GetMoodPlaylistsQuery, GetWatchPlaylistQuery};

/// YouTube Music's "My Supermix": a mix made from the signed-in account's listening.
#[cfg(feature = "youtube")]
const SUPERMIX_ID: &str = "RDTMAK5uy_kset8DisdE7LSD4TNjEVvrKRTmG7a56sY";

/// Run the same request on the signed-in or the signed-out client.
#[cfg(feature = "youtube")]
macro_rules! with_client {
    ($inner:expr, $c:ident => $call:expr) => {
        match $inner {
            YouTubeClientInner::Authenticated($c) => $call,
            YouTubeClientInner::Unauthenticated($c) => $call,
        }
    };
}

#[cfg(feature = "youtube")]
fn watch_track_item(track: WatchPlaylistTrack) -> BrowseItem {
    let vid = track.video_id.get_raw().to_string();
    BrowseItem {
        id: format!("yt:song:{}", vid),
        title: track.title,
        subtitle: Some(track.author),
        kind: BrowseItemKind::Track,
        is_container: false,
        track_ref: Some(TrackRef::YouTube(vid)),
        duration_secs: parse_duration_str(&track.duration),
        artwork_url: select_thumbnail(&track.thumbnails),
        album: None,
        depth: 0,
    }
}

#[cfg(feature = "youtube")]
/// The playlists and albums of a mood or genre page, each subtitled with its shelf.
/// Read from the page itself: genre pages have a layout ytmapi-rs cannot parse.
fn mood_page_items(page: &serde_json::Value) -> Vec<BrowseItem> {
    fn text(value: &serde_json::Value) -> String {
        value["runs"]
            .as_array()
            .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect())
            .unwrap_or_default()
    }
    fn walk(value: &serde_json::Value, shelf: &str, out: &mut Vec<BrowseItem>) {
        match value {
            serde_json::Value::Object(map) => {
                // A shelf names the cards below it
                let header = &value["header"];
                let title = text(&header["musicCarouselShelfBasicHeaderRenderer"]["title"]);
                let title = if title.is_empty() {
                    text(&header["gridHeaderRenderer"]["title"])
                } else {
                    title
                };
                let shelf = if title.is_empty() { shelf } else { &title };

                if let Some(card) = map.get("musicTwoRowItemRenderer") {
                    let browse_id = card["navigationEndpoint"]["browseEndpoint"]["browseId"]
                        .as_str()
                        .unwrap_or("");
                    let id = if let Some(playlist) = browse_id.strip_prefix("VL") {
                        format!("yt:playlist:{}", playlist)
                    } else if browse_id.starts_with("MPRE") {
                        format!("yt:album:{}", browse_id)
                    } else {
                        return;
                    };
                    let thumbnails = &card["thumbnailRenderer"]["musicThumbnailRenderer"]
                        ["thumbnail"]["thumbnails"];
                    let cover = thumbnails.as_array().and_then(|t| {
                        t.iter()
                            .max_by_key(|t| t["width"].as_u64().unwrap_or(0))
                            .and_then(|t| t["url"].as_str())
                            .map(full_size_cover)
                    });
                    let by = text(&card["subtitle"]);
                    let subtitle = match (shelf.is_empty(), by.is_empty()) {
                        (false, false) => format!("{} • {}", shelf, by),
                        (false, true) => shelf.to_string(),
                        _ => by,
                    };
                    let mut item = folder(id, text(&card["title"]), &subtitle, cover);
                    if item.id.starts_with("yt:album:") {
                        item.kind = BrowseItemKind::Album;
                    }
                    out.push(item);
                    return;
                }
                for child in map.values() {
                    walk(child, shelf, out);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    walk(child, shelf, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(page, "", &mut out);
    out
}

#[cfg(feature = "youtube")]
fn folder(id: String, title: String, subtitle: &str, artwork_url: Option<String>) -> BrowseItem {
    BrowseItem {
        id,
        title,
        subtitle: Some(subtitle.to_string()),
        kind: BrowseItemKind::Playlist,
        is_container: true,
        track_ref: None,
        duration_secs: None,
        artwork_url,
        album: None,
        depth: 0,
    }
}
#[cfg(feature = "youtube")]
use ytmapi_rs::parse::PlaylistItem;
#[cfg(feature = "youtube")]
use ytmapi_rs::YtMusic;

#[cfg(feature = "youtube")]
pub fn select_thumbnail(thumbnails: &[Thumbnail]) -> Option<String> {
    thumbnails
        .iter()
        .max_by_key(|t| t.width)
        .map(|t| t.url.clone())
}

#[cfg(feature = "youtube")]
pub enum YouTubeClientInner {
    Authenticated(YtMusic<BrowserToken>),
    Unauthenticated(YtMusic<NoAuthToken>),
}

#[cfg(feature = "youtube")]
pub struct YouTubeClient {
    inner: Option<YouTubeClientInner>,
    /// The saved sign-in was found signed out when the client started.
    expired: bool,
}

#[cfg(feature = "youtube")]
impl YouTubeClient {
    pub async fn new(cookie: Option<&str>) -> Self {
        let mut expired = false;
        if let Some(cookie) = cookie {
            // A cookie Google has signed out is still taken by `from_cookie`; the
            // library then fails. Signed out, search and playback work without it.
            if crate::sources::youtube_login::is_signed_in(cookie).await == Some(false) {
                log::warn!("The saved YouTube Music sign-in has expired");
                expired = true;
            } else if let Ok(client) = YtMusic::from_cookie(cookie).await {
                return Self {
                    inner: Some(YouTubeClientInner::Authenticated(client)),
                    expired,
                };
            }
        }

        let unauth = YtMusic::new_unauthenticated().await.ok();
        Self {
            inner: unauth.map(YouTubeClientInner::Unauthenticated),
            expired,
        }
    }

    /// True when the saved sign-in was found signed out.
    pub fn sign_in_expired(&self) -> bool {
        self.expired
    }

    pub fn is_authenticated(&self) -> bool {
        matches!(self.inner, Some(YouTubeClientInner::Authenticated(_)))
    }

    pub async fn login(&mut self, cookie: &str) -> Result<String, String> {
        if crate::sources::youtube_login::is_signed_in(cookie).await == Some(false) {
            return Err(
                "This cookie is signed out. Press Enter to sign in in a browser window instead."
                    .to_string(),
            );
        }
        match YtMusic::from_cookie(cookie).await {
            Ok(client) => {
                self.inner = Some(YouTubeClientInner::Authenticated(client));
                self.expired = false;
                Ok("Connected to YouTube Music".to_string())
            }
            Err(e) => Err(format!("Failed to authenticate with cookie: {}", e)),
        }
    }

    /// Search songs, albums and playlists. These are three separate searches:
    /// YouTube Music's combined search no longer returns anything usable.
    pub async fn search(&self, query: &str) -> Result<Vec<BrowseItem>, String> {
        let Some(ref inner) = self.inner else {
            return Err("YouTube Music client not initialized".to_string());
        };

        // Independent requests, so they run together
        macro_rules! search_all {
            ($client:expr) => {
                tokio::join!(
                    $client.search_songs(query),
                    $client.search_albums(query),
                    $client.search_playlists(query),
                )
            };
        }
        let (songs, albums, playlists) = match inner {
            YouTubeClientInner::Authenticated(c) => search_all!(c),
            YouTubeClientInner::Unauthenticated(c) => search_all!(c),
        };
        if let (Err(e), Err(_), Err(_)) = (&songs, &albums, &playlists) {
            return Err(e.to_string());
        }
        // One kind failing must not hide what the others found
        fn found<T>(kind: &str, result: Result<Vec<T>, ytmapi_rs::Error>) -> Vec<T> {
            result.unwrap_or_else(|e| {
                log::warn!("YouTube {} search failed: {}", kind, e);
                Vec::new()
            })
        }

        let mut items = Vec::new();

        for song in found("song", songs) {
            let vid = song.video_id.get_raw().to_string();
            items.push(BrowseItem {
                id: format!("yt:song:{}", vid),
                title: song.title,
                subtitle: Some(song.artist),
                kind: BrowseItemKind::Track,
                is_container: false,
                track_ref: Some(TrackRef::YouTube(vid)),
                duration_secs: parse_duration_str(&song.duration),
                artwork_url: select_thumbnail(&song.thumbnails),
                album: song.album.map(|a| a.name).filter(|a| !a.is_empty()),
                depth: 0,
            });
        }

        for album in found("album", albums) {
            let aid = album.album_id.get_raw().to_string();
            items.push(BrowseItem {
                id: format!("yt:album:{}", aid),
                title: album.title,
                subtitle: Some(format!("{} • {}", album.artist, album.year)),
                kind: BrowseItemKind::Album,
                is_container: true,
                track_ref: None,
                duration_secs: None,
                artwork_url: select_thumbnail(&album.thumbnails),
                album: None,
                depth: 0,
            });
        }

        for playlist in found("playlist", playlists) {
            use ytmapi_rs::parse::SearchResultPlaylist;
            let (title, author, playlist_id, thumbnails) = match playlist {
                SearchResultPlaylist::Featured(p) => {
                    (p.title, p.author, p.playlist_id, p.thumbnails)
                }
                SearchResultPlaylist::Community(p) => {
                    (p.title, p.author, p.playlist_id, p.thumbnails)
                }
                // Podcasts are not listed
                _ => continue,
            };
            items.push(BrowseItem {
                id: format!("yt:playlist:{}", playlist_id.get_raw()),
                title,
                subtitle: Some(format!("Playlist • {}", author)),
                kind: BrowseItemKind::Playlist,
                is_container: true,
                track_ref: None,
                duration_secs: None,
                artwork_url: select_thumbnail(&thumbnails),
                album: None,
                depth: 0,
            });
        }

        Ok(items)
    }

    /// YouTube Music's radio for a song: similar songs, starting with it.
    pub async fn radio(&self, video_id: &str) -> Result<Vec<BrowseItem>, String> {
        let Some(ref inner) = self.inner else {
            return Err("YouTube Music client not initialized".to_string());
        };
        let tracks = with_client!(inner, c => c
            .get_watch_playlist_from_video_id(VideoID::from_raw(video_id))
            .await)
        .map_err(|e| e.to_string())?;
        Ok(tracks.into_iter().map(watch_track_item).collect())
    }

    pub fn library_roots(&self) -> Vec<BrowseItem> {
        if self.is_authenticated() {
            vec![
                folder(
                    "yt:made_for_you".into(),
                    "Made for you".into(),
                    "Your mixes, moods and genres",
                    None,
                ),
                BrowseItem {
                    id: "yt:liked_songs".into(),
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
                    id: "yt:library_playlists".into(),
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
                    id: "yt:library_albums".into(),
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
                id: "yt:login_hint".into(),
                title: "Sign in to YouTube Music".into(),
                subtitle: Some("Press Enter, or search without signing in".into()),
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

    pub async fn children(&self, parent_id: &str) -> Result<Vec<BrowseItem>, String> {
        let Some(ref inner) = self.inner else {
            return Err("YouTube Music client not initialized".to_string());
        };

        if parent_id == "yt:made_for_you" {
            return Ok(vec![
                folder(
                    format!("yt:watch:{}", SUPERMIX_ID),
                    "My Supermix".into(),
                    "Mix • made from your listening",
                    None,
                ),
                folder(
                    "yt:moods:0".into(),
                    "Moods & moments".into(),
                    "Playlists for a mood",
                    None,
                ),
                folder(
                    "yt:moods:1".into(),
                    "Genres".into(),
                    "Playlists by genre",
                    None,
                ),
            ]);
        }

        // A mix or radio: a playlist YouTube Music builds when it is played
        if let Some(pid) = parent_id.strip_prefix("yt:watch:") {
            let query = GetWatchPlaylistQuery::new_from_playlist_id(PlaylistID::from_raw(pid));
            let tracks =
                with_client!(inner, c => c.query(query).await).map_err(|e| e.to_string())?;
            return Ok(tracks.into_iter().map(watch_track_item).collect());
        }

        if let Some(vid) = parent_id.strip_prefix("yt:radio:") {
            return self.radio(vid).await;
        }

        if let Some(section) = parent_id.strip_prefix("yt:moods:") {
            let sections = with_client!(inner, c => c.get_mood_categories().await)
                .map_err(|e| e.to_string())?;
            let index: usize = section.parse().unwrap_or(0);
            return Ok(sections
                .into_iter()
                .nth(index)
                .map(|s| s.mood_categories)
                .unwrap_or_default()
                .into_iter()
                .map(|category| {
                    folder(
                        format!("yt:mood:{}", category.params.get_raw()),
                        category.title,
                        "Playlists",
                        None,
                    )
                })
                .collect());
        }

        if let Some(params) = parent_id.strip_prefix("yt:mood:") {
            let query = GetMoodPlaylistsQuery::new(MoodCategoryParams::from_raw(params));
            let page = with_client!(inner, c => c.raw_json_query(query).await)
                .map_err(|e| e.to_string())?;
            let page: serde_json::Value = serde_json::from_str(&page).map_err(|e| e.to_string())?;
            return Ok(mood_page_items(&page));
        }

        if parent_id == "yt:liked_songs" {
            let YouTubeClientInner::Authenticated(ref c) = inner else {
                return Err("Authentication required for Liked Songs".to_string());
            };
            let songs = c.get_library_songs().await.map_err(|e| e.to_string())?;
            return Ok(songs
                .into_iter()
                .map(|song| {
                    let vid = song.video_id.get_raw().to_string();
                    let artist_names: Vec<String> =
                        song.artists.into_iter().map(|a| a.name).collect();
                    BrowseItem {
                        id: format!("yt:song:{}", vid),
                        title: song.title,
                        subtitle: Some(artist_names.join(", ")),
                        kind: BrowseItemKind::Track,
                        is_container: false,
                        track_ref: Some(TrackRef::YouTube(vid)),
                        duration_secs: parse_duration_str(&song.duration),
                        artwork_url: select_thumbnail(&song.thumbnails),
                        album: Some(song.album.name).filter(|a| !a.is_empty()),
                        depth: 0,
                    }
                })
                .collect());
        }

        if parent_id == "yt:library_playlists" {
            let YouTubeClientInner::Authenticated(ref c) = inner else {
                return Err("Authentication required for Library Playlists".to_string());
            };
            let playlists = c.get_library_playlists().await.map_err(|e| e.to_string())?;
            return Ok(playlists
                .into_iter()
                .map(|pl| {
                    let pid = pl.playlist_id.get_raw().to_string();
                    BrowseItem {
                        id: format!("yt:playlist:{}", pid),
                        title: pl.title,
                        subtitle: Some(format!("Playlist • {}", pl.author)),
                        kind: BrowseItemKind::Playlist,
                        is_container: true,
                        track_ref: None,
                        duration_secs: None,
                        artwork_url: select_thumbnail(&pl.thumbnails),
                        album: None,
                        depth: 0,
                    }
                })
                .collect());
        }

        if parent_id == "yt:library_albums" {
            let YouTubeClientInner::Authenticated(ref c) = inner else {
                return Err("Authentication required for Library Albums".to_string());
            };
            let albums = c.get_library_albums().await.map_err(|e| e.to_string())?;
            return Ok(albums
                .into_iter()
                .map(|alb| {
                    let aid = alb.album_id.get_raw().to_string();
                    BrowseItem {
                        id: format!("yt:album:{}", aid),
                        title: alb.title,
                        subtitle: Some(format!("{} • {}", alb.artist, alb.year)),
                        kind: BrowseItemKind::Album,
                        is_container: true,
                        track_ref: None,
                        duration_secs: None,
                        artwork_url: select_thumbnail(&alb.thumbnails),
                        album: None,
                        depth: 0,
                    }
                })
                .collect());
        }

        if let Some(pid) = parent_id.strip_prefix("yt:playlist:") {
            // YouTube Music opens a playlist by its browse id: the id after "VL"
            let browse_id = if pid.starts_with("VL") {
                pid.to_string()
            } else {
                format!("VL{}", pid)
            };
            let tracks = match with_client!(inner, c => c
                .get_playlist_tracks(PlaylistID::from_raw(browse_id.as_str()))
                .await)
            {
                Ok(tracks) => tracks,
                // Some of YouTube Music's own playlists have a page ytmapi-rs cannot
                // read; played as a mix they list their tracks
                Err(e) => {
                    let mix = format!("yt:watch:{}", browse_id.trim_start_matches("VL"));
                    log::info!("Opening {} as a mix: {}", parent_id, e);
                    return Box::pin(self.children(&mix))
                        .await
                        .map_err(|_| e.to_string());
                }
            };

            let mut items = Vec::new();
            for item in tracks {
                match item {
                    PlaylistItem::Song(s) => {
                        let vid = s.video_id.get_raw().to_string();
                        let artists: Vec<String> = s.artists.into_iter().map(|a| a.name).collect();
                        items.push(BrowseItem {
                            id: format!("yt:song:{}", vid),
                            title: s.title,
                            subtitle: Some(artists.join(", ")),
                            kind: BrowseItemKind::Track,
                            is_container: false,
                            track_ref: Some(TrackRef::YouTube(vid)),
                            duration_secs: parse_duration_str(&s.duration),
                            artwork_url: select_thumbnail(&s.thumbnails),
                            album: Some(s.album.name).filter(|a| !a.is_empty()),
                            depth: 0,
                        });
                    }
                    PlaylistItem::Video(v) => {
                        let vid = v.video_id.get_raw().to_string();
                        items.push(BrowseItem {
                            id: format!("yt:song:{}", vid),
                            title: v.title,
                            subtitle: Some(v.channel_name),
                            kind: BrowseItemKind::Track,
                            is_container: false,
                            track_ref: Some(TrackRef::YouTube(vid)),
                            duration_secs: parse_duration_str(&v.duration),
                            artwork_url: select_thumbnail(&v.thumbnails),
                            album: None,
                            depth: 0,
                        });
                    }
                    _ => {}
                }
            }
            return Ok(items);
        }

        if let Some(aid) = parent_id.strip_prefix("yt:album:") {
            let album = match inner {
                YouTubeClientInner::Authenticated(c) => c
                    .get_album(AlbumID::from_raw(aid))
                    .await
                    .map_err(|e| e.to_string())?,
                YouTubeClientInner::Unauthenticated(c) => c
                    .get_album(AlbumID::from_raw(aid))
                    .await
                    .map_err(|e| e.to_string())?,
            };

            let album_artist = album
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let album_cover = select_thumbnail(&album.thumbnails);

            let mut items = Vec::new();
            for song in album.tracks {
                let vid = song.video_id.get_raw().to_string();
                items.push(BrowseItem {
                    id: format!("yt:song:{}", vid),
                    title: song.title,
                    subtitle: if album_artist.is_empty() {
                        None
                    } else {
                        Some(album_artist.clone())
                    },
                    kind: BrowseItemKind::Track,
                    is_container: false,
                    track_ref: Some(TrackRef::YouTube(vid)),
                    duration_secs: parse_duration_str(&song.duration),
                    artwork_url: album_cover.clone(),
                    album: Some(album.title.clone()),
                    depth: 0,
                });
            }
            return Ok(items);
        }

        Ok(Vec::new())
    }
}

#[cfg(not(feature = "youtube"))]
pub struct YouTubeClient;

#[cfg(not(feature = "youtube"))]
impl YouTubeClient {
    pub async fn new(_cookie: Option<&str>) -> Self {
        Self
    }
    pub fn is_authenticated(&self) -> bool {
        false
    }
    pub fn sign_in_expired(&self) -> bool {
        false
    }
    pub async fn login(&mut self, _cookie: &str) -> Result<String, String> {
        Err("YouTube feature not enabled in build".to_string())
    }
    pub async fn search(&self, _query: &str) -> Result<Vec<BrowseItem>, String> {
        Err("YouTube feature not enabled in build".to_string())
    }
    pub fn library_roots(&self) -> Vec<BrowseItem> {
        Vec::new()
    }
    pub async fn children(&self, _parent_id: &str) -> Result<Vec<BrowseItem>, String> {
        Err("YouTube feature not enabled in build".to_string())
    }
    pub async fn radio(&self, _video_id: &str) -> Result<Vec<BrowseItem>, String> {
        Err("YouTube feature not enabled in build".to_string())
    }
}

impl crate::sources::MusicSource for YouTubeClient {
    fn name(&self) -> &'static str {
        "YouTube Music"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_requested_in_full_size() {
        assert_eq!(
            full_size_cover("https://yt3.googleusercontent.com/AbC-d_e=w120-h120-l90-rj"),
            "https://yt3.googleusercontent.com/AbC-d_e=w544-h544-l90-rj"
        );
        assert_eq!(
            full_size_cover("https://lh3.googleusercontent.com/x=w60-h60"),
            "https://lh3.googleusercontent.com/x=w544-h544"
        );
        // Other addresses, such as video stills and Spotify covers, are left alone
        for url in [
            "https://i.ytimg.com/vi/abc/sddefault.jpg?sqp=x&rs=AMzJL3k=w1",
            "https://i.scdn.co/image/ab67616d0000b273",
            "https://yt3.googleusercontent.com/x=s120",
            "https://yt3.googleusercontent.com/x=w-h120",
        ] {
            assert_eq!(full_size_cover(url), url);
        }
    }

    #[test]
    fn test_parse_duration_str() {
        assert_eq!(parse_duration_str("45"), Some(45));
        assert_eq!(parse_duration_str("3:45"), Some(225));
        assert_eq!(parse_duration_str("03:45"), Some(225));
        assert_eq!(parse_duration_str("1:02:15"), Some(3735));
        assert_eq!(parse_duration_str(""), None);
        assert_eq!(parse_duration_str("invalid"), None);
    }
}
