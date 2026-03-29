use std::path::PathBuf;

use anyhow::{Context, Result};

fn parse_pack_paths(value: &str) -> Vec<PathBuf> {
    value
        .split(',')
        .map(|segment| PathBuf::from(segment.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let host = std::env::var("MEMKIT_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("PORT")
        .or_else(|_| std::env::var("MEMKIT_PORT"))
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);
    let packs = std::env::var("MEMKIT_PACKS")
        .ok()
        .map(|value| parse_pack_paths(&value))
        .unwrap_or_default();

    memkit::server::run_server(packs, host, port)
        .await
        .context("memkit server failed")
}
