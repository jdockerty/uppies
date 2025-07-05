use clap::Parser;

use prometheus::Registry;
use uppies::{ping_targets, PingSender, Result};

#[derive(Debug, Parser)]
struct Cli {
    /// Targets that should have pings sent to them.
    targets: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let metrics = Registry::default();

    let sender = PingSender::new(cli.targets, &metrics)?;
    ping_targets(sender).await;
    tokio::signal::ctrl_c().await?;

    Ok(())
}
