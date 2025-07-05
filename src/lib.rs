use std::{net::IpAddr, str::FromStr, time::Duration};

use surge_ping::{Client, Config, PingIdentifier, PingSequence};

pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

/// A dispatcher to send pings (ICMP packets) to a specified target.
pub struct Dispatcher {
    target: String,
    client: Client,
}

impl Dispatcher {
    pub fn new(target: String) -> Result<Self> {
        let client = surge_ping::Client::new(&Config::new())?;
        Ok(Self { target, client })
    }

    pub async fn run(self) -> Result<()> {
        let mut pinger = self
            .client
            .pinger(
                IpAddr::from_str(&self.target)?,
                PingIdentifier(rand::random()),
            )
            .await;

        let mut interval = tokio::time::interval(Duration::from_millis(10));
        loop {
            interval.tick().await;
            let (_, duration) = pinger.ping(PingSequence(0), &[]).await?;
            println!("{}: {duration:?}", self.target);
        }
    }
}
