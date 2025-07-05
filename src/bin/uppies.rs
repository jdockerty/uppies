use clap::Parser;

use uppies::{Dispatcher, Result};

#[derive(Debug, Parser)]
struct Cli {
    targets: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    for target in cli.targets {
        let dispatcher = Dispatcher::new(target.clone())?;
        tokio::spawn(dispatcher.run());
    }

    tokio::signal::ctrl_c().await?;

    Ok(())
}
