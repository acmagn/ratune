//! Thin binary wrapper around [`ratune::run`].

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    ratune::run().await
}
