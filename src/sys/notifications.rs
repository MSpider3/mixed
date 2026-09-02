//! Desktop notifications for mixed.
//!
//! Sends native desktop notifications on track changes via D-Bus (`org.freedesktop.Notifications`).

#[cfg(target_os = "linux")]
pub fn show_notification(title: &str, artist: &str, album: &str) {
    let title = title.to_string();
    let artist = artist.to_string();
    let album = album.to_string();

    std::thread::spawn(move || {
        let body = if !album.is_empty() && album != "Unknown Album" {
            format!("{}\n{}", artist, album)
        } else {
            artist
        };

        // Use a lightweight, synchronous or async one-shot D-Bus connection
        if let Ok(connection) = zbus::blocking::Connection::session() {
            let hints = std::collections::HashMap::<&str, zbus::zvariant::Value>::new();
            let actions: Vec<&str> = Vec::new();
            let _ = connection.call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "mixed",
                    0u32,
                    "audio-player",
                    &title,
                    &body,
                    actions,
                    hints,
                    3500i32, // 3.5 seconds
                ),
            );
        }
    });
}

#[cfg(not(target_os = "linux"))]
pub fn show_notification(_title: &str, _artist: &str, _album: &str) {}
