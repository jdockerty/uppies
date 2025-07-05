use clap::Parser;

use prometheus::Registry;
use uppies::{Dispatcher, Result};

#[derive(Debug, Parser)]
struct Cli {
    /// Targets that should have pings sent to them.
    targets: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let metrics = Registry::default();

    for target in cli.targets {
        let dispatcher = Dispatcher::new(target.clone(), &metrics)?;
        tokio::spawn(dispatcher.run());
    }

    // TODO: include HTTP server to serve metrics path
    tokio::signal::ctrl_c().await?;

    Ok(())
}
