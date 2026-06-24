//! Probe radio station icon URL build + HTTP fetch + image decode (no TUI).
//!
//! ```text
//! cargo run -p ratune --example radio_art_probe -- https://www.yourclassical.org
//! ```

use anyhow::{Context, Result};
use ratune_subsonic::InternetRadioStation;

fn main() -> Result<()> {
    let homepage = std::env::args()
        .nth(1)
        .context("usage: radio_art_probe <homepage-url> [stream-url]")?;
    let stream_url = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "http://127.0.0.1/stream".into());

    let station = InternetRadioStation {
        id: "probe".into(),
        name: "probe".into(),
        stream_url,
        home_page_url: Some(homepage.clone()),
        cover_art: None,
    };

    println!("homepage:   {homepage}");
    println!("cache key:  {}", station.art_cache_key());
    for url in station.station_icon_urls() {
        println!("candidate:  {url}");
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(ratune_subsonic::client::USER_AGENT)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let mut best: Option<(String, usize, u32, u32, u32)> = None;
        for url in station.station_icon_urls() {
            let resp = client.get(&url).send().await;
            let Ok(resp) = resp else {
                println!("{url}: request failed");
                continue;
            };
            if !resp.status().is_success() {
                println!("{url}: HTTP {}", resp.status());
                continue;
            }
            let bytes = resp.bytes().await?.to_vec();
            let Some(img) = image::load_from_memory(&bytes).ok() else {
                println!("{url}: {} bytes, decode failed", bytes.len());
                continue;
            };
            let area = img.width().saturating_mul(img.height());
            println!(
                "{url}: {} bytes, {}x{}",
                bytes.len(),
                img.width(),
                img.height()
            );
            if best
                .as_ref()
                .map(|(_, _, _, _, best_area)| area > *best_area)
                .unwrap_or(true)
            {
                best = Some((url, bytes.len(), img.width(), img.height(), area));
            }
        }

        let Some((url, nbytes, w, h, _)) = best else {
            anyhow::bail!("no decodable icon from homepage");
        };
        println!("picked:     {url} ({nbytes} bytes, {w}x{h})");
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}
