use std::time::{Duration, Instant};

fn probe(url: &str) {
    eprintln!("\n=== {url} ===");
    let t0 = Instant::now();
    let open = ratune_player::stream::open_live_stream(url);
    eprintln!(
        "open ({:?}): {:?}",
        t0.elapsed(),
        open.as_ref().map(|_| "ok").map_err(|e| e.to_string())
    );
    let Ok(reader) = open else { return };
    let t1 = Instant::now();
    let dec = rodio::Decoder::builder().with_data(reader).build();
    eprintln!(
        "decode ({:?}): {:?}",
        t1.elapsed(),
        dec.as_ref().map(|_| "ok").map_err(|e| e.to_string())
    );
    if let Ok(mut d) = dec {
        let t2 = Instant::now();
        let s = d.next();
        eprintln!("first sample ({:?}): {:?}", t2.elapsed(), s.is_some());
    }
}

#[test]
#[ignore = "requires network"]
fn probe_common_radio_formats() {
    probe("http://stream.dancewave.online:8080/dance.mp3");
    probe("https://stream.4zzz.org.au:9200/4zzz");
    probe("http://ice1.somafm.com/groovesalad-128-mp3");
}
