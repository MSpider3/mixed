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
use ytmapi_rs::common::{AlbumID, PlaylistID, Thumbnail, YoutubeID};
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
}

#[cfg(feature = "youtube")]
impl YouTubeClient {
    pub async fn new(cookie: Option<&str>) -> Self {
        if let Some(cookie) = cookie {
            if let Ok(client) = YtMusic::from_cookie(cookie).await {
                return Self {
                    inner: Some(YouTubeClientInner::Authenticated(client)),
                };
            }
        }

        let unauth = YtMusic::new_unauthenticated().await.ok();
        Self {
            inner: unauth.map(YouTubeClientInner::Unauthenticated),
        }
    }

    pub fn is_authenticated(&self) -> bool {
        matches!(self.inner, Some(YouTubeClientInner::Authenticated(_)))
    }

    pub async fn login(&mut self, cookie: &str) -> Result<String, String> {
        match YtMusic::from_cookie(cookie).await {
            Ok(client) => {
                self.inner = Some(YouTubeClientInner::Authenticated(client));
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

    pub fn library_roots(&self) -> Vec<BrowseItem> {
        if self.is_authenticated() {
            vec![
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
                title: "Login with YouTube Music cookie".into(),
                subtitle: Some("Press Enter to input cookie or search directly".into()),
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
            let tracks = match inner {
                YouTubeClientInner::Authenticated(c) => c
                    .get_playlist_tracks(PlaylistID::from_raw(pid))
                    .await
                    .map_err(|e| e.to_string())?,
                YouTubeClientInner::Unauthenticated(c) => c
                    .get_playlist_tracks(PlaylistID::from_raw(pid))
                    .await
                    .map_err(|e| e.to_string())?,
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
