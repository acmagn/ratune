use std::time::{Duration, Instant};

use ratune_player::{spawn_player, PlayerCommand, PlayerEvent};

#[test]
#[ignore = "requires network"]
fn spawn_player_plays_live_stream() {
    let (tx, rx, handle, _samples) = spawn_player();
    tx.send(PlayerCommand::PlayLiveStream {
        url: "http://stream.dancewave.online:8080/dance.mp3".into(),
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
        panic!("player error: {e}");
    }
    assert!(started, "TrackStarted never received within 30s");
}
