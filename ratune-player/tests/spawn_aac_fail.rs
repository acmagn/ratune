use ratune_player::{spawn_player, PlayerCommand, PlayerEvent};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires network"]
fn spawn_player_reports_decode_error_for_unsupported() {
    let (tx, rx, handle, _) = spawn_player();
    tx.send(PlayerCommand::PlayLiveStream {
        url: "https://stream.4zzz.org.au:9200/4zzz".into(),
        gen: 1,
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut err = None;
    while Instant::now() < deadline {
        while let Ok(ev) = rx.try_recv() {
            if let PlayerEvent::Error(e) = ev {
                err = Some(e);
            }
        }
        if err.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    tx.send(PlayerCommand::Quit).unwrap();
    let _ = handle.join();
    eprintln!("error: {:?}", err);
    assert!(err.is_some(), "expected Error event within 45s");
}
