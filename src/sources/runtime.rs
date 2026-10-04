use super::{SourceEvent, SourceRequest, SourceTab};
use crossbeam_channel::{Receiver, Sender};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

/// Background network runtime controller.
pub struct SourceRuntime {
    request_tx: Sender<SourceRequest>,
    event_rx: Receiver<SourceEvent>,
    shutdown: Arc<AtomicBool>,
}

impl SourceRuntime {
    /// Spawn a new background network runtime thread with default configuration.
    pub fn spawn() -> Self {
        Self::spawn_with_config(&crate::config::app_config::AppConfig::default())
    }

    /// Spawn a new background network runtime thread with explicit configuration.
    pub fn spawn_with_config(config: &crate::config::app_config::AppConfig) -> Self {
        let (request_tx, request_rx) = crossbeam_channel::unbounded::<SourceRequest>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<SourceEvent>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let creds = crate::config::credentials::Credentials::load();
        let yt_cookie = creds.youtube_cookie.clone();
        let sp_client_id = Some(crate::sources::spotify::DEFAULT_SPOTIFY_CLIENT_ID.to_string());
        let (sp_access, sp_refresh) = creds
            .spotify_token_cache
            .as_deref()
            .map(|cache_str| {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(cache_str) {
                    let acc = val
                        .get("access_token")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(ToString::to_string);
                    let ref_tok = val
                        .get("refresh_token")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(ToString::to_string);
                    (acc, ref_tok)
                } else if !cache_str.trim().is_empty() {
                    (Some(cache_str.to_string()), None)
                } else {
                    (None, None)
                }
            })
            .unwrap_or((None, None));

        let yt_cache_mb = config.yt_cache_mb;
        let yt_dlp_path = config.yt_dlp_path.clone();

        // Evict stale YouTube cache at startup
        crate::sources::ytdlp::YtDlp::evict_cache(yt_cache_mb);

        thread::Builder::new()
            .name("mixed-net-runtime".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = event_tx.send(SourceEvent::Error {
                            source: SourceTab::Unified,
                            error: format!("Failed to initialize async network runtime: {}", e),
                        });
                        return;
                    }
                };

                rt.block_on(async move {
                    let yt_client = Arc::new(tokio::sync::Mutex::new(
                        crate::sources::youtube::YouTubeClient::new(yt_cookie.as_deref()).await,
                    ));
                    let spotify_client = Arc::new(tokio::sync::Mutex::new(
                        crate::sources::spotify::SpotifyClient::new(
                            sp_client_id,
                            sp_access,
                            sp_refresh,
                        )
                        .await,
                    ));
                    let ytdlp = crate::sources::ytdlp::YtDlp::new(yt_dlp_path);
                    let in_flight_downloads: InFlightDownloads = Arc::default();

                    while !shutdown_clone.load(Ordering::Relaxed) {
                        // Receive request or poll shutdown every 100ms
                        let req =
                            match request_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                                Ok(req) => req,
                                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                            };

                        let event_tx = event_tx.clone();
                        let yt_client = yt_client.clone();
                        let spotify_client = spotify_client.clone();
                        let ytdlp = ytdlp.clone();
                        let in_flight = in_flight_downloads.clone();
                        tokio::spawn(async move {
                            handle_request(
                                req,
                                event_tx,
                                yt_client,
                                spotify_client,
                                ytdlp,
                                in_flight,
                                yt_cache_mb,
                            )
                            .await;
                        });
                    }
                });
            })
            .expect("Failed to spawn mixed-net-runtime thread");

        Self {
            request_tx,
            event_rx,
            shutdown,
        }
    }

    /// Send a request to the background runtime.
    pub fn send(
        &self,
        req: SourceRequest,
    ) -> Result<(), crossbeam_channel::SendError<SourceRequest>> {
        self.request_tx.send(req)
    }

    /// Borrow the incoming event receiver for select! in main event loop.
    pub fn event_rx(&self) -> &Receiver<SourceEvent> {
        &self.event_rx
    }
}

impl Drop for SourceRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// A YouTube download that has not finished yet.
struct InFlightDownload {
    progress: Arc<crate::audio::growing_file::DownloadProgress>,
    /// The partial file, once enough of it exists to start playback.
    streamable: Option<PathBuf>,
}

/// Downloads currently running, keyed by video id. Guarded by a std mutex: it is
/// only held for map updates, never across an await.
type InFlightDownloads = Arc<std::sync::Mutex<std::collections::HashMap<String, InFlightDownload>>>;

#[cfg(feature = "spotify")]
const SPOTIFY_SEARCH_PAGE_SIZE: usize = crate::sources::spotify::SEARCH_PAGE_SIZE;
#[cfg(not(feature = "spotify"))]
const SPOTIFY_SEARCH_PAGE_SIZE: usize = 10;

async fn handle_request(
    req: SourceRequest,
    event_tx: Sender<SourceEvent>,
    yt_client: Arc<tokio::sync::Mutex<crate::sources::youtube::YouTubeClient>>,
    spotify_client: Arc<tokio::sync::Mutex<crate::sources::spotify::SpotifyClient>>,
    ytdlp: crate::sources::ytdlp::YtDlp,
    in_flight_downloads: InFlightDownloads,
    yt_cache_mb: usize,
) {
    match req {
        SourceRequest::CheckAuth(source) => match source {
            SourceTab::Spotify => {
                let sp = spotify_client.lock().await;
                let connected = sp.is_authenticated();
                let user_name = sp.user_name().map(|s| s.to_string());
                let error = if !connected {
                    sp.last_error().map(|s| s.to_string())
                } else {
                    None
                };
                let _ = event_tx.send(SourceEvent::AuthState {
                    source,
                    connected,
                    user_name,
                    error,
                });
                let items = sp.library_roots();
                let _ = event_tx.send(SourceEvent::Roots { source, items });
            }
            SourceTab::YouTube => {
                let yt = yt_client.lock().await;
                let connected = yt.is_authenticated();
                let user_name = if connected {
                    Some("YouTube Music".to_string())
                } else {
                    None
                };
                let _ = event_tx.send(SourceEvent::AuthState {
                    source,
                    connected,
                    user_name,
                    error: None,
                });
                let items = yt.library_roots();
                let _ = event_tx.send(SourceEvent::Roots { source, items });
            }
            _ => {}
        },
        SourceRequest::Login { source, payload } => match source {
            SourceTab::Spotify => {
                let mut sp = spotify_client.lock().await;
                match sp.login(&payload).await {
                    Ok(user_info) => {
                        let mut creds = crate::config::credentials::Credentials::load();
                        creds.spotify_client_id = sp.client_id().map(ToString::to_string);
                        if let Some(tok) = sp.access_token() {
                            let cache = serde_json::json!({
                                "access_token": tok,
                                "refresh_token": sp.refresh_token(),
                            });
                            creds.spotify_token_cache = Some(cache.to_string());
                        }
                        let _ = creds.save();
                        let _ = event_tx.send(SourceEvent::AuthState {
                            source,
                            connected: true,
                            user_name: sp.user_name().map(|s| s.to_string()).or(Some(user_info)),
                            error: None,
                        });
                        let roots = sp.library_roots();
                        let _ = event_tx.send(SourceEvent::Roots {
                            source,
                            items: roots,
                        });
                    }
                    Err(err) => {
                        let _ = event_tx.send(SourceEvent::AuthState {
                            source,
                            connected: false,
                            user_name: None,
                            error: Some(err),
                        });
                    }
                }
            }
            SourceTab::YouTube => {
                let mut yt = yt_client.lock().await;
                match yt.login(&payload).await {
                    Ok(_) => {
                        let mut creds = crate::config::credentials::Credentials::load();
                        creds.youtube_cookie = Some(payload);
                        let _ = creds.save();
                        let _ = event_tx.send(SourceEvent::AuthState {
                            source,
                            connected: true,
                            user_name: Some("YouTube Music".to_string()),
                            error: None,
                        });
                        let roots = yt.library_roots();
                        let _ = event_tx.send(SourceEvent::Roots {
                            source,
                            items: roots,
                        });
                    }
                    Err(err) => {
                        let _ = event_tx.send(SourceEvent::AuthState {
                            source,
                            connected: false,
                            user_name: None,
                            error: Some(err),
                        });
                    }
                }
            }
            _ => {}
        },
        SourceRequest::FetchRoots(source) => match source {
            SourceTab::Spotify => {
                let sp = spotify_client.lock().await;
                let items = sp.library_roots();
                let _ = event_tx.send(SourceEvent::Roots { source, items });
            }
            SourceTab::YouTube => {
                let yt = yt_client.lock().await;
                let items = yt.library_roots();
                let _ = event_tx.send(SourceEvent::Roots { source, items });
            }
            _ => {}
        },
        SourceRequest::FetchChildren {
            source,
            parent_id,
            page,
        } => match source {
            SourceTab::Spotify => {
                let mut sp = spotify_client.lock().await;
                sp.ensure_fresh_token().await;
                if let Err(e) = sp
                    .stream_children(&parent_id, event_tx.clone(), source)
                    .await
                {
                    let _ = event_tx.send(SourceEvent::Error { source, error: e });
                }
            }
            SourceTab::YouTube => {
                let yt = yt_client.lock().await;
                match yt.children(&parent_id).await {
                    Ok(items) => {
                        let _ = event_tx.send(SourceEvent::Children {
                            source,
                            parent_id,
                            items,
                            page,
                            has_more: false,
                        });
                    }
                    Err(e) => {
                        let _ = event_tx.send(SourceEvent::Error { source, error: e });
                    }
                }
            }
            _ => {}
        },
        SourceRequest::Search {
            source,
            query,
            generation,
            page,
        } => {
            if query.trim().is_empty() {
                let _ = event_tx.send(SourceEvent::SearchResults {
                    source,
                    query,
                    generation,
                    items: Vec::new(),
                    page,
                    has_more: false,
                });
                return;
            }
            match source {
                SourceTab::Spotify => {
                    let mut sp = spotify_client.lock().await;
                    sp.ensure_fresh_token().await;
                    match sp.search(&query, page).await {
                        Ok(items) => {
                            // A full page of any one result type means there may be more.
                            let has_more = items.len() >= SPOTIFY_SEARCH_PAGE_SIZE;
                            let _ = event_tx.send(SourceEvent::SearchResults {
                                source,
                                query,
                                generation,
                                items,
                                page,
                                has_more,
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(SourceEvent::Error { source, error: e });
                        }
                    }
                }
                SourceTab::YouTube => {
                    let yt = yt_client.lock().await;
                    match yt.search(&query).await {
                        Ok(items) => {
                            let _ = event_tx.send(SourceEvent::SearchResults {
                                source,
                                query,
                                generation,
                                items,
                                page,
                                has_more: false,
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(SourceEvent::Error { source, error: e });
                        }
                    }
                }
                _ => {}
            }
        }
        SourceRequest::FetchCover { url } => {
            if let Some(path) = fetch_and_cache_cover(&url).await {
                let _ = event_tx.send(SourceEvent::CoverFetched { url, path });
            }
        }
        SourceRequest::DownloadYouTubeTrack {
            video_id,
            generation,
        } => {
            let progress = {
                let Ok(mut active) = in_flight_downloads.lock() else {
                    return;
                };
                if let Some(running) = active.get(&video_id) {
                    // Already downloading (e.g. as a prefetch). If it is playable by
                    // now, say so again for this request; otherwise the running
                    // download reports it when it gets there.
                    if let Some(path) = running.streamable.clone() {
                        let _ = event_tx.send(SourceEvent::YouTubeTrackStreamable {
                            video_id,
                            path,
                            progress: running.progress.clone(),
                        });
                    }
                    return;
                }
                let progress = Arc::new(crate::audio::growing_file::DownloadProgress::default());
                active.insert(
                    video_id.clone(),
                    InFlightDownload {
                        progress: progress.clone(),
                        streamable: None,
                    },
                );
                progress
            };

            let format = "bestaudio[ext=m4a]/bestaudio[ext=mp3]/bestaudio[acodec^=mp4a]/bestaudio[acodec^=mp3]/bestaudio[ext=aac]";
            let res = ytdlp
                .download_audio(&video_id, format, &progress, |path| {
                    if let Ok(mut active) = in_flight_downloads.lock() {
                        if let Some(running) = active.get_mut(&video_id) {
                            running.streamable = Some(path.clone());
                        }
                    }
                    let _ = event_tx.send(SourceEvent::YouTubeTrackStreamable {
                        video_id: video_id.clone(),
                        path,
                        progress: progress.clone(),
                    });
                })
                .await;
            if let Ok(mut active) = in_flight_downloads.lock() {
                active.remove(&video_id);
            }

            match res {
                Ok(path) => {
                    crate::sources::ytdlp::YtDlp::evict_cache(yt_cache_mb);
                    let _ = event_tx.send(SourceEvent::YouTubeTrackDownloaded {
                        video_id,
                        path,
                        generation,
                    });
                }
                Err(error) => {
                    let _ = event_tx.send(SourceEvent::YouTubeDownloadFailed {
                        video_id,
                        error,
                        generation,
                    });
                }
            }
        }
    }
}

async fn fetch_and_cache_cover(url: &str) -> Option<PathBuf> {
    #[cfg(any(feature = "spotify", feature = "youtube"))]
    {
        use crate::utils::hash::fnv1a_hex;
        let cover_hash = fnv1a_hex(url);
        let mut cache_dir = directories::ProjectDirs::from("", "", "mixed")
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        cache_dir.push("covers");
        let _ = tokio::fs::create_dir_all(&cache_dir).await;
        let file_path = cache_dir.join(format!("{}.jpg", cover_hash));

        if file_path.exists() {
            return Some(file_path);
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .ok()?;
        let resp = client.get(url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let bytes = resp.bytes().await.ok()?;
        let tmp_path = file_path.with_extension("tmp");
        if tokio::fs::write(&tmp_path, &bytes).await.is_ok() {
            let _ = tokio::fs::rename(&tmp_path, &file_path).await;
            return Some(file_path);
        }
        None
    }
    #[cfg(not(any(feature = "spotify", feature = "youtube")))]
    {
        let _ = url;
        None
    }
}
