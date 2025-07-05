use std::{net::IpAddr, str::FromStr, time::Duration};

use prometheus::{IntCounter, Registry};
use surge_ping::{Client, Config, PingIdentifier, PingSequence};

pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

pub struct PingSender {
    dispatchers: Vec<Dispatcher>,
}

impl PingSender {
    pub fn new(targets: Vec<String>, metrics: &Registry) -> Result<Self> {
        Ok(Self {
            dispatchers: targets
                .iter()
                .map(|t| Dispatcher::new(t.clone(), &metrics))
                .collect::<Result<_>>()?,
        })
    }
}

/// Start pinging all targets configured within the [`PingSender`]
pub async fn ping_targets(sender: PingSender) {
    for d in sender.dispatchers {
        tokio::spawn(d.run());
    }
}

/// A dispatcher to send pings (ICMP packets) to a specified target.
struct Dispatcher {
    target: String,
    client: Client,

    success_count: IntCounter,
    failure_count: IntCounter,
}

impl Dispatcher {
    fn new(target: String, metrics: &Registry) -> Result<Self> {
        let client = surge_ping::Client::new(&Config::new())?;

        // TODO: this will fail running multiple.
        //
        // Fix this by placing metrics at the top-level for overall successful/failed pings.
        let success_count = IntCounter::new("dispatcher_success_count", "TODO")?;
        let failure_count = IntCounter::new("dispatcher_failure_count", "TODO")?;
        metrics.register(Box::new(success_count.clone())).unwrap();
        metrics.register(Box::new(failure_count.clone())).unwrap();
        Ok(Self {
            target,
            client,
            success_count,
            failure_count,
        })
    }

    /// Run this dispatcher, performing the ping operation to the given target.
    ///
    /// This is a blocking call and will perform continuous pings against
    /// the target.
    async fn run(self) -> Result<()> {
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
            match pinger.ping(PingSequence(0), &[]).await {
                Ok((_, duration)) => {
                    println!("{}: {duration:?}", self.target);
                    self.success_count.inc();
                }
                Err(e) => {
                    eprintln!("{e}");
                    self.failure_count.inc()
                }
            }
        }
    }
}
