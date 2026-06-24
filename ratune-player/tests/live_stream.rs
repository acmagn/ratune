use std::time::{Duration, Instant};

#[test]
#[ignore = "requires network"]
fn live_stream_opens_and_decodes() {
    let url = "http://stream.dancewave.online:8080/dance.mp3";
    let start = Instant::now();
    let reader = ratune_player::stream::open_live_stream(url).expect("open_live_stream");
    eprintln!("prebuffer in {:?}", start.elapsed());

    let start = Instant::now();
    let mut decoder = rodio::Decoder::builder()
        .with_data(reader)
        .build()
        .expect("decoder build");
    eprintln!("decoder build in {:?}", start.elapsed());

    let start = Instant::now();
    let sample = decoder.next();
    eprintln!("first sample in {:?}: {:?}", start.elapsed(), sample.is_some());
    assert!(sample.is_some());
    assert!(start.elapsed() < Duration::from_secs(30));
}

#[test]
#[ignore = "requires network"]
fn live_stream_decodes_past_prebuffer() {
    let url = "http://stream.dancewave.online:8080/dance.mp3";
    let reader = ratune_player::stream::open_live_stream(url).expect("open");
    let mut decoder = rodio::Decoder::builder()
        .with_data(reader)
        .build()
        .expect("decode");

    let start = Instant::now();
    let mut samples = 0u64;
    while start.elapsed() < Duration::from_secs(5) {
        if decoder.next().is_some() {
            samples += 1;
        }
    }
    eprintln!("samples in 5s: {samples}");
    assert!(
        samples > 100_000,
        "decoder stalled after prebuffer (only {samples} samples in 5s)"
    );
}

