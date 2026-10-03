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

    #[allow(deprecated)]
    pub async fn search(&self, query: &str) -> Result<Vec<BrowseItem>, String> {
        let Some(ref inner) = self.inner else {
            return Err("YouTube Music client not initialized".to_string());
        };

        let results = match inner {
            YouTubeClientInner::Authenticated(c) => {
                c.search(query).await.map_err(|e| e.to_string())?
            }
            YouTubeClientInner::Unauthenticated(c) => {
                c.search(query).await.map_err(|e| e.to_string())?
            }
        };

        let mut items = Vec::new();

        // 1. Songs
        for song in results.songs {
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
                depth: 0,
            });
        }

        // 2. Albums
        for album in results.albums {
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
                depth: 0,
            });
        }

        // 3. Featured Playlists
        for pl in results.featured_playlists {
            let pid = pl.playlist_id.get_raw().to_string();
            items.push(BrowseItem {
                id: format!("yt:playlist:{}", pid),
                title: pl.title,
                subtitle: Some(format!("Playlist • {}", pl.author)),
                kind: BrowseItemKind::Playlist,
                is_container: true,
                track_ref: None,
                duration_secs: None,
                artwork_url: select_thumbnail(&pl.thumbnails),
                depth: 0,
            });
        }

        // 4. Community Playlists
        for item in results.community_playlists {
            if let ytmapi_rs::parse::BasicSearchResultCommunityPlaylist::Playlist(pl) = item {
                let pid = pl.playlist_id.get_raw().to_string();
                items.push(BrowseItem {
                    id: format!("yt:playlist:{}", pid),
                    title: pl.title,
                    subtitle: Some(format!("Playlist • {}", pl.author)),
                    kind: BrowseItemKind::Playlist,
                    is_container: true,
                    track_ref: None,
                    duration_secs: None,
                    artwork_url: select_thumbnail(&pl.thumbnails),
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
                    id: "yt:liked_songs".into(),
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
                    id: "yt:library_playlists".into(),
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
                    id: "yt:library_albums".into(),
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
                id: "yt:login_hint".into(),
                title: "Login with YouTube Music cookie".into(),
                subtitle: Some("Press Enter to input cookie or search directly".into()),
                kind: BrowseItemKind::Playlist,
                is_container: false,
                track_ref: None,
                duration_secs: None,
                artwork_url: None,
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
    fn test_parse_duration_str() {
        assert_eq!(parse_duration_str("45"), Some(45));
        assert_eq!(parse_duration_str("3:45"), Some(225));
        assert_eq!(parse_duration_str("03:45"), Some(225));
        assert_eq!(parse_duration_str("1:02:15"), Some(3735));
        assert_eq!(parse_duration_str(""), None);
        assert_eq!(parse_duration_str("invalid"), None);
    }
}
