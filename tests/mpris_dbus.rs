use std::time::Duration;

use mixed::sys::mpris::{self, MprisCommand};
use zbus::{fdo::DBusProxy, Connection};

const MPRIS_BUS_NAME: &str = "org.mpris.MediaPlayer2.mixed";

async fn player_proxy() -> zbus::Result<zbus::Proxy<'static>> {
    zbus::Proxy::new(
        &Connection::session().await?,
        MPRIS_BUS_NAME,
        "/org/mpris/MediaPlayer2",
        "org.mpris.MediaPlayer2.Player",
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mpris_methods_dispatch_commands() {
    if let Err(err) = Connection::session().await {
        eprintln!("skipping MPRIS DBus test: no usable session bus: {err}");
        return;
    }

    let (tx, rx) = crossbeam_channel::unbounded();
    let (_state, _updates) = mpris::start_mpris(tx);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let conn = Connection::session()
                .await
                .expect("session bus connection failed");
            let dbus = DBusProxy::new(&conn).await.expect("DBus proxy failed");
            if dbus
                .name_has_owner(MPRIS_BUS_NAME.try_into().expect("valid bus name"))
                .await
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("MPRIS service did not appear on session bus");

    let proxy = player_proxy().await.expect("player proxy failed");

    proxy
        .call::<_, _, ()>("PlayPause", &())
        .await
        .expect("PlayPause call failed");
    proxy
        .call::<_, _, ()>("Next", &())
        .await
        .expect("Next call failed");
    proxy
        .call::<_, _, ()>("Previous", &())
        .await
        .expect("Previous call failed");

    let first = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("PlayPause command was not delivered");
    let second = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("Next command was not delivered");
    let third = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("Previous command was not delivered");

    assert!(matches!(first, MprisCommand::PlayPause));
    assert!(matches!(second, MprisCommand::Next));
    assert!(matches!(third, MprisCommand::Previous));
}
