use std::time::{Duration, Instant};

use ratune_player::{spawn_player, PlayerCommand, PlayerEvent};

#[test]
#[ignore = "requires network"]
fn aac_adts_stream_decodes_with_hint() {
    let url = "http://18073.live.streamtheworld.com:80/WQHTFMAAC_SC";
    let mut reader = ratune_player::stream::open_live_stream(url).expect("open");
    let hint = reader.prepare_for_decode(url);
    assert_eq!(hint.format, ratune_player::stream::StreamFormatHint::Aac);

    let reader = ratune_player::adts_normalize::AdtsNormalizeReader::new(reader);
    let t0 = Instant::now();
    let mut builder = rodio::Decoder::builder()
        .with_data(reader)
        .with_coarse_seek(true)
        .with_hint("aac")
        .with_mime_type("audio/aac");
    let mut decoder = builder.build().expect("decode");
    eprintln!("decode in {:?}", t0.elapsed());
    assert!(decoder.next().is_some());
}

#[test]
#[ignore = "requires network"]
fn spawn_player_plays_aac_stream() {
    let (tx, rx, handle, _) = spawn_player();
    tx.send(PlayerCommand::PlayLiveStream {
        url: "http://18073.live.streamtheworld.com:80/WQHTFMAAC_SC".into(),
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
    assert!(started, "TrackStarted never received for AAC stream");
}

#[test]
#[ignore = "requires network"]
fn mpeg2_adts_stream_plays() {
    let (tx, rx, handle, _) = spawn_player();
    tx.send(PlayerCommand::PlayLiveStream {
        url: "http://ubuntu.hbr1.com:19800/ambient.aac".into(),
        gen: 1,
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(45);
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
    assert!(started, "MPEG-2 ADTS stream should play");
}
