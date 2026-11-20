//! Independent, ephemeral document scenario. The catalog application is unchanged.

#[cfg(feature = "http")]
#[path = "browser_concept/mod.rs"]
mod browser_concept;

#[cfg(feature = "http")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    browser_concept::run().await
}

#[cfg(not(feature = "http"))]
fn main() {
    eprintln!("browser-concept requires the http feature");
    std::process::exit(64);
}
