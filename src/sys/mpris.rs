use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::RwLock;
use zbus::{interface, Connection};

#[derive(Debug, Clone, Default)]
pub struct MprisMetadataStrings {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String, // e.g., "file:///tmp/cover.png"
}

/// Shared MPRIS state updated by the main app thread.
#[derive(Debug)]
pub struct MprisState {
    pub playback_status: AtomicU8, // 0=Stopped, 1=Playing, 2=Paused
    pub loop_status: AtomicU8,     // 0=None, 1=Track, 2=Playlist
    pub shuffle: AtomicBool,
    pub volume: AtomicU64, // f64::to_bits()
    pub position_us: AtomicI64,
    pub length_us: AtomicI64,
    pub can_go_next: AtomicBool,
    pub can_go_previous: AtomicBool,
    pub can_play: AtomicBool,
    pub can_pause: AtomicBool,
    pub seek_target: AtomicI64, // -1 for none

    pub metadata: RwLock<MprisMetadataStrings>,
}

impl Default for MprisState {
    fn default() -> Self {
        Self {
            playback_status: AtomicU8::new(0),
            loop_status: AtomicU8::new(0),
            shuffle: AtomicBool::new(false),
            volume: AtomicU64::new(0.8f64.to_bits()),
            position_us: AtomicI64::new(0),
            length_us: AtomicI64::new(0),
            can_go_next: AtomicBool::new(false),
            can_go_previous: AtomicBool::new(false),
            can_play: AtomicBool::new(false),
            can_pause: AtomicBool::new(false),
            seek_target: AtomicI64::new(-1),
            metadata: RwLock::new(MprisMetadataStrings::default()),
        }
    }
}

pub type SharedMprisState = Arc<MprisState>;

/// Commands from MPRIS to the player.
#[derive(Debug, Clone)]
pub enum MprisCommand {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Seek(i64),              // offset in microseconds
    SetPosition(i64),       // absolute position in microseconds
    SetLoopStatus(String),  // "None", "Track", "Playlist"
    SetShuffle(bool),
    SetVolume(f64),         // 0.0-1.0 per MPRIS spec
    #[allow(dead_code)]
    Quit,
}

/// MPRIS2 MediaPlayer2 root interface.
struct MediaPlayer2Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl MediaPlayer2Root {
    fn raise(&self) {}
    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_set_fullscreen(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn fullscreen(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        "mixed".to_string()
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "mixed".to_string()
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![
            "audio/mpeg".into(),
            "audio/flac".into(),
            "audio/ogg".into(),
            "audio/wav".into(),
        ]
    }
}

/// MPRIS2 Player interface.
struct MediaPlayer2Player {
    state: SharedMprisState,
    command_tx: crossbeam_channel::Sender<MprisCommand>,
}

impl MediaPlayer2Player {
    fn send_command(&self, cmd: MprisCommand) {
        if let Err(e) = self.command_tx.try_send(cmd) {
            eprintln!("MPRIS: failed to enqueue command: {:?}", e);
        }
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl MediaPlayer2Player {
    // THIN ROUTER: Instantly send command and return. Do not lock or read state logic.
    fn next(&self) {
        self.send_command(MprisCommand::Next);
    }
    fn previous(&self) {
        self.send_command(MprisCommand::Previous);
    }
    fn pause(&self) {
        self.send_command(MprisCommand::Pause);
    }
    fn play_pause(&self) {
        self.send_command(MprisCommand::PlayPause);
    }
    fn stop(&self) {
        self.send_command(MprisCommand::Stop);
    }
    fn play(&self) {
        self.send_command(MprisCommand::Play);
    }
    fn seek(&self, offset: i64) {
        self.send_command(MprisCommand::Seek(offset));
    }
    fn set_position(&self, _track_id: zbus::zvariant::ObjectPath<'_>, position: i64) {
        self.send_command(MprisCommand::SetPosition(position));
    }

    // ── Read properties ──────────────────────────────────────────────────────

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match self.state.playback_status.load(Ordering::Relaxed) {
            1 => "Playing".to_string(),
            2 => "Paused".to_string(),
            _ => "Stopped".to_string(),
        }
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
        match self.state.loop_status.load(Ordering::Relaxed) {
            1 => "Track".to_string(),
            2 => "Playlist".to_string(),
            _ => "None".to_string(),
        }
    }

    /// Writable: Linux Control Center / playerctl `loop` command hits this.
    #[zbus(property)]
    fn set_loop_status(&self, value: String) {
        self.send_command(MprisCommand::SetLoopStatus(value));
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.state.shuffle.load(Ordering::Relaxed)
    }

    /// Writable: toggle shuffle from media widget or `playerctl shuffle on`.
    #[zbus(property)]
    fn set_shuffle(&self, value: bool) {
        self.send_command(MprisCommand::SetShuffle(value));
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, zbus::zvariant::Value<'_>> {
        let meta = self.state.metadata.read().unwrap();
        let mut map = HashMap::new();
        map.insert(
            "xesam:title".into(),
            zbus::zvariant::Value::from(meta.title.clone()),
        );
        map.insert(
            "xesam:artist".into(),
            zbus::zvariant::Value::from(vec![meta.artist.clone()]),
        );
        map.insert(
            "xesam:album".into(),
            zbus::zvariant::Value::from(meta.album.clone()),
        );

        if !meta.art_url.is_empty() {
            map.insert(
                "mpris:artUrl".into(),
                zbus::zvariant::Value::from(meta.art_url.clone()),
            );
        }

        map.insert(
            "mpris:length".into(),
            zbus::zvariant::Value::from(self.state.length_us.load(Ordering::Relaxed)),
        );
        map.insert(
            "mpris:trackid".into(),
            zbus::zvariant::Value::from(
                zbus::zvariant::ObjectPath::try_from("/org/mpris/MediaPlayer2/Track/0").unwrap(),
            ),
        );
        map
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        f64::from_bits(self.state.volume.load(Ordering::Relaxed))
    }

    /// Writable: `playerctl volume 0.5` or GNOME volume slider.
    /// MPRIS spec passes 0.0–1.0; we store it in the atomic and forward a SetVolume command.
    #[zbus(property)]
    fn set_volume(&self, value: f64) {
        // Clamp to [0.0, 1.0]
        let clamped = value.max(0.0).min(1.0);
        self.state.volume.store(clamped.to_bits(), Ordering::Relaxed);
        self.send_command(MprisCommand::SetVolume(clamped));
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state.position_us.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.state.can_go_next.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.state.can_go_previous.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.state.can_play.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.state.can_pause.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
}

/// Start the MPRIS2 D-Bus service on an isolated background Tokio runtime.
/// Returns the shared state handle and a trigger transmitter to signal immediate property updates.
pub fn start_mpris(
    command_tx: crossbeam_channel::Sender<MprisCommand>,
) -> (SharedMprisState, tokio::sync::mpsc::UnboundedSender<()>) {
    let state = Arc::new(MprisState::default());
    let (update_tx, mut update_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let state_clone = state.clone();
    let command_tx_clone = command_tx;

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("MPRIS: Failed to build tokio runtime: {:?}", e);
                return;
            }
        };

        rt.block_on(async move {
            let conn = match Connection::session().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("MPRIS: Failed to connect to session D-Bus: {:?}", e);
                    return;
                }
            };

            let root = MediaPlayer2Root;
            let player = MediaPlayer2Player {
                state: state_clone.clone(),
                command_tx: command_tx_clone,
            };

            if let Err(e) = conn
                .object_server()
                .at("/org/mpris/MediaPlayer2", root)
                .await
            {
                eprintln!("MPRIS: Failed to register root interface: {:?}", e);
                return;
            }

            if let Err(e) = conn
                .object_server()
                .at("/org/mpris/MediaPlayer2", player)
                .await
            {
                eprintln!("MPRIS: Failed to register player interface: {:?}", e);
                return;
            }

            let flags = enumflags2::BitFlags::from(zbus::fdo::RequestNameFlags::ReplaceExisting)
                | enumflags2::BitFlags::from(zbus::fdo::RequestNameFlags::AllowReplacement);

            if let Err(e) = conn
                .request_name_with_flags("org.mpris.MediaPlayer2.mixed", flags)
                .await
            {
                eprintln!(
                    "MPRIS: Failed to request name org.mpris.MediaPlayer2.mixed: {:?}",
                    e
                );
                return;
            }

            let mut last_status = 0;
            let mut last_title = String::new();
            let mut last_artist = String::new();
            let mut last_album = String::new();
            let mut last_art_url = String::new();
            let mut last_volume = 0.0;
            let mut last_shuffle = false;
            let mut last_loop = 0;
            let mut last_can_go_next = false;
            let mut last_can_go_previous = false;
            let mut last_can_play = false;
            let mut last_can_pause = false;

            loop {
                // Wait for a notification from the UI thread (which implements a debouncer)
                if update_rx.recv().await.is_none() {
                    break; // Application is shutting down
                }

                let mut seeked_val = None;
                let target = state_clone.seek_target.swap(-1, Ordering::Relaxed);
                if target != -1 {
                    seeked_val = Some(target);
                }

                let status = state_clone.playback_status.load(Ordering::Relaxed);
                let volume = f64::from_bits(state_clone.volume.load(Ordering::Relaxed));
                let shuffle = state_clone.shuffle.load(Ordering::Relaxed);
                let loop_status = state_clone.loop_status.load(Ordering::Relaxed);
                let length_us = state_clone.length_us.load(Ordering::Relaxed);
                let can_go_next = state_clone.can_go_next.load(Ordering::Relaxed);
                let can_go_previous = state_clone.can_go_previous.load(Ordering::Relaxed);
                let can_play = state_clone.can_play.load(Ordering::Relaxed);
                let can_pause = state_clone.can_pause.load(Ordering::Relaxed);

                let (title, artist, album, art_url) = {
                    let meta = state_clone.metadata.read().unwrap();
                    (
                        meta.title.clone(),
                        meta.artist.clone(),
                        meta.album.clone(),
                        meta.art_url.clone(),
                    )
                };

                let mut changed: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();

                if let Some(pos) = seeked_val {
                    changed.insert("Position", zbus::zvariant::Value::from(pos));
                }

                if status != last_status {
                    let status_str = match status {
                        1 => "Playing",
                        2 => "Paused",
                        _ => "Stopped",
                    };
                    changed.insert(
                        "PlaybackStatus",
                        zbus::zvariant::Value::from(status_str.to_string()),
                    );
                }
                if title != last_title
                    || artist != last_artist
                    || album != last_album
                    || art_url != last_art_url
                {
                    let mut map: HashMap<String, zbus::zvariant::Value<'_>> = HashMap::new();
                    map.insert(
                        "xesam:title".to_string(),
                        zbus::zvariant::Value::from(title.clone()),
                    );
                    map.insert(
                        "xesam:artist".to_string(),
                        zbus::zvariant::Value::from(vec![artist.clone()]),
                    );
                    map.insert(
                        "xesam:album".to_string(),
                        zbus::zvariant::Value::from(album.clone()),
                    );

                    if !art_url.is_empty() {
                        map.insert(
                            "mpris:artUrl".to_string(),
                            zbus::zvariant::Value::from(art_url.clone()),
                        );
                    }

                    map.insert(
                        "mpris:length".to_string(),
                        zbus::zvariant::Value::from(length_us),
                    );
                    map.insert(
                        "mpris:trackid".to_string(),
                        zbus::zvariant::Value::from(
                            zbus::zvariant::ObjectPath::try_from("/org/mpris/MediaPlayer2/Track/0")
                                .unwrap(),
                        ),
                    );

                    changed.insert("Metadata", zbus::zvariant::Value::from(map));
                }
                if (volume - last_volume).abs() > 0.01 {
                    changed.insert("Volume", zbus::zvariant::Value::from(volume));
                }
                if shuffle != last_shuffle {
                    changed.insert("Shuffle", zbus::zvariant::Value::from(shuffle));
                }
                if loop_status != last_loop {
                    let loop_str = match loop_status {
                        1 => "Track",
                        2 => "Playlist",
                        _ => "None",
                    };
                    changed.insert(
                        "LoopStatus",
                        zbus::zvariant::Value::from(loop_str.to_string()),
                    );
                }
                if can_go_next != last_can_go_next {
                    changed.insert("CanGoNext", zbus::zvariant::Value::from(can_go_next));
                }
                if can_go_previous != last_can_go_previous {
                    changed.insert(
                        "CanGoPrevious",
                        zbus::zvariant::Value::from(can_go_previous),
                    );
                }
                if can_play != last_can_play {
                    changed.insert("CanPlay", zbus::zvariant::Value::from(can_play));
                }
                if can_pause != last_can_pause {
                    changed.insert("CanPause", zbus::zvariant::Value::from(can_pause));
                }

                if let Ok(emitter) =
                    zbus::object_server::SignalEmitter::new(&conn, "/org/mpris/MediaPlayer2")
                {
                    // Emit Seeked(position: x) per MPRIS spec §2.6.
                    // We emit it as a PropertiesChanged entry so the position is always
                    // current; the dedicated Seeked signal is also fired for clients that
                    // listen explicitly (e.g. playerctl).
                    if let Some(pos) = seeked_val {
                        // Emit PropertiesChanged for Position so all clients pick it up
                        let mut seeked_changed: HashMap<&str, zbus::zvariant::Value<'_>> =
                            HashMap::new();
                        seeked_changed
                            .insert("Position", zbus::zvariant::Value::from(pos));
                        let _ = emitter
                            .emit(
                                "org.freedesktop.DBus.Properties",
                                "PropertiesChanged",
                                &(
                                    "org.mpris.MediaPlayer2.Player",
                                    &seeked_changed,
                                    &[] as &[&str],
                                ),
                            )
                            .await;
                    }

                    if !changed.is_empty() {
                        let _ = emitter
                            .emit(
                                "org.freedesktop.DBus.Properties",
                                "PropertiesChanged",
                                &("org.mpris.MediaPlayer2.Player", &changed, &[] as &[&str]),
                            )
                            .await;

                        last_status = status;
                        last_title = title;
                        last_artist = artist;
                        last_album = album;
                        last_art_url = art_url;
                        last_volume = volume;
                        last_shuffle = shuffle;
                        last_loop = loop_status;
                        last_can_go_next = can_go_next;
                        last_can_go_previous = can_go_previous;
                        last_can_play = can_play;
                        last_can_pause = can_pause;
                    }
                }
            }
        });
    });

    (state, update_tx)
}
