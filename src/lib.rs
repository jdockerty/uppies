use std::{net::IpAddr, str::FromStr};

use surge_ping::{Client, Config, PingIdentifier, PingSequence};

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

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
            .pinger(IpAddr::from_str(&self.target)?, PingIdentifier(10))
            .await;

        loop {
            let (_, duration) = pinger.ping(PingSequence(0), &[]).await?;
            println!("{duration:?}");
        }
    }
}
