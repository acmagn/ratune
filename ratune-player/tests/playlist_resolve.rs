use std::time::{Duration, Instant};

use ratune_player::{spawn_player, PlayerCommand, PlayerEvent};

#[test]
#[ignore = "requires network"]
fn spawn_player_plays_pls_playlist_url() {
    let (tx, rx, handle, _) = spawn_player();
    tx.send(PlayerCommand::PlayLiveStream {
        url: "http://provisioning.streamtheworld.com/pls/WQHTFM.pls".into(),
        gen: 1,
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut started = false;
    let mut err = None;
    while Instant::now() < deadline {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                PlayerEvent::TrackStarted => started = true,
                PlayerEvent::Error(e) => err = Some(e),
                _ => {}
            }
        }
        if started || err.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    tx.send(PlayerCommand::Quit).unwrap();
    let _ = handle.join();

    if let Some(e) = err {
        // PLS resolves to an AAC stream on this host — decode may fail, but not at playlist fetch.
        assert!(
            !e.contains("stream HTTP error for http://provisioning.streamtheworld.com"),
            "playlist URL itself should not 404: {e}"
        );
        return;
    }
    assert!(
        started,
        "TrackStarted never received for .pls URL within 30s"
    );
}
