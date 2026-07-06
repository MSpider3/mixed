/// Android media controls bridge using `termux-api` as a proxy.
///
/// Architecture:
///   App state ──termux-notification──→ Android notification bar
///                                              │ button tap
///   Main loop ←──crossbeam channel──── UDS Listener Thread
///                                      $PREFIX/tmp/mixed.sock
///
/// The notification shows ⏮ / ⏯ / ⏭ buttons. Each button writes a short
/// command string to the Unix domain socket, which this module reads and
/// maps to `MediaCommand` values sent to the app's event channel.
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::MediaCommand;

// ── Socket path ──────────────────────────────────────────────────────────────

/// Returns the Unix domain socket path, honouring Termux's $PREFIX layout.
/// Falls back to /tmp if $PREFIX is not set (shouldn't happen in Termux).
fn socket_path() -> PathBuf {
    let prefix = std::env::var("PREFIX").unwrap_or_else(|_| "/data/data/com.termux/files/usr".into());
    PathBuf::from(format!("{}/tmp/mixed.sock", prefix))
}

// ── Public handle ─────────────────────────────────────────────────────────────

/// Handle returned by `start_android_media`. Used to push metadata updates
/// and to cleanly shut down the notification + listener on exit.
pub struct AndroidMediaHandle {
    shutdown: Arc<AtomicBool>,
    sock: PathBuf,
}

impl AndroidMediaHandle {
    /// Push current track metadata to the Android notification bar.
    /// Spawned with `.spawn()` so it never blocks the UI thread.
    pub fn push_metadata(&self, title: &str, artist: &str) {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }

        let sock = self.sock.to_string_lossy().into_owned();
        let title = shell_escape(title);
        let artist = shell_escape(artist);

        // Build button actions that write commands to the socket
        let prev_action   = format!("echo prev > '{sock}'");
        let pp_action     = format!("echo playpause > '{sock}'");
        let next_action   = format!("echo next > '{sock}'");
        let delete_action = format!("echo quit > '{sock}'");

        let _ = std::process::Command::new("termux-notification")
            .args([
                "--id",            "mixed_player",
                "--type",          "media",
                "--title",         &title,
                "--content",       &artist,
                "--button1",       "⏮",
                "--button1-action", &prev_action,
                "--button2",       "⏯",
                "--button2-action", &pp_action,
                "--button3",       "⏭",
                "--button3-action", &next_action,
                "--on-delete",     &delete_action,
            ])
            .spawn();
    }

    /// Signal the listener thread to stop and clean up the Android notification.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);

        // Remove the lock-screen / notification widget
        let _ = std::process::Command::new("termux-notification-remove")
            .arg("mixed_player")
            .spawn();

        // Delete the socket file so the next launch can bind cleanly
        cleanup_socket(&self.sock);
    }
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Start the Android media bridge. Creates the UDS listener, spawns the
/// background reader thread, and returns an `AndroidMediaHandle`.
///
/// # Panic Safety
/// The socket file is removed **before** binding, so a previous crash that
/// left a stale `mixed.sock` behind will never cause an "Address already in use"
/// error on the next launch.
pub fn start_android_media(
    command_tx: crossbeam_channel::Sender<MediaCommand>,
) -> AndroidMediaHandle {
    let sock = socket_path();
    let shutdown = Arc::new(AtomicBool::new(false));

    // ── Stale socket cleanup (SIGKILL / crash recovery) ──────────────────────
    // Always remove the socket before binding. This is the user's comment:
    // "Ensure the IDE's initialization logic explicitly runs remove_file()
    //  before attempting to bind, otherwise it will panic with 'Address
    //  already in use'."
    cleanup_socket(&sock);

    // ── Bind listener ────────────────────────────────────────────────────────
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("android_media: failed to bind socket {:?}: {}", sock, e);
            return AndroidMediaHandle { shutdown, sock };
        }
    };

    // ── Spawn listener thread ─────────────────────────────────────────────────
    let shutdown_clone = shutdown.clone();
    let sock_clone = sock.clone();

    std::thread::spawn(move || {
        // Set a short accept timeout so the thread can periodically check the
        // shutdown flag even when no client connects.
        let _ = listener.set_nonblocking(true);

        loop {
            if shutdown_clone.load(Ordering::Relaxed) {
                cleanup_socket(&sock_clone);
                break;
            }

            match listener.accept() {
                Ok((stream, _)) => {
                    let reader = BufReader::new(stream);
                    for line in reader.lines() {
                        let cmd_str = match line {
                            Ok(s) => s.trim().to_ascii_lowercase(),
                            Err(_) => break,
                        };

                        let cmd = match cmd_str.as_str() {
                            "playpause" => Some(MediaCommand::PlayPause),
                            "play"      => Some(MediaCommand::Play),
                            "pause"     => Some(MediaCommand::Pause),
                            "next"      => Some(MediaCommand::Next),
                            "prev" | "previous" => Some(MediaCommand::Previous),
                            "stop"      => Some(MediaCommand::Stop),
                            "quit"      => Some(MediaCommand::Quit),
                            _           => None,
                        };

                        if let Some(c) = cmd {
                            if command_tx.send(c).is_err() {
                                // Main loop has exited; stop listener
                                cleanup_socket(&sock_clone);
                                return;
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No client yet — sleep briefly and retry
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    eprintln!("android_media: accept error: {}", e);
                    break;
                }
            }
        }
    });

    AndroidMediaHandle { shutdown, sock }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn cleanup_socket(path: &PathBuf) {
    // Silently ignore errors (file may not exist on first launch)
    let _ = std::fs::remove_file(path);
}

/// Minimal shell escaping: replaces single quotes so they are safe inside
/// single-quoted shell strings used in termux-notification button actions.
fn shell_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}
