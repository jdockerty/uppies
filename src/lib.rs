use std::{net::IpAddr, str::FromStr, time::Duration};

use prometheus::{IntCounterVec, Opts, Registry};
use surge_ping::{Client, Config, PingIdentifier, PingSequence};
use tokio::sync::mpsc::{error::TryRecvError, Receiver, Sender};

pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

pub struct PingSender {
    dispatchers: Vec<(Dispatcher, Receiver<Result<()>>)>,

    success_count: IntCounterVec,
    failure_count: IntCounterVec,
}

impl PingSender {
    const LABELS: &[&str] = &["targets"];

    pub fn new(targets: Vec<String>, metrics: &Registry) -> Result<Self> {
        let success_count = IntCounterVec::new(
            Opts::new("ping_success_count", "Counter of successful pings"),
            Self::LABELS,
        )?;
        let failure_count = IntCounterVec::new(
            Opts::new("ping_failure_count", "Counter of failed pings"),
            Self::LABELS,
        )?;
        metrics.register(Box::new(success_count.clone()))?;
        metrics.register(Box::new(failure_count.clone()))?;
        Ok(Self {
            dispatchers: targets
                .iter()
                .map(|t| Dispatcher::new(t.clone()))
                .collect::<Result<_>>()?,
            success_count,
            failure_count,
        })
    }
}

/// Start pinging all targets configured within the [`PingSender`]
pub async fn ping_targets(sender: PingSender) {
    for (dispatcher, mut rx) in sender.dispatchers {
        let success_count = sender.success_count.clone();
        let failure_count = sender.failure_count.clone();

        let target = dispatcher.target.clone();
        tokio::spawn(dispatcher.run());
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            loop {
                interval.tick().await;
                match rx.try_recv() {
                    Ok(res) => match res {
                        Ok(_) => success_count.with_label_values(&[target.clone()]).inc(),
                        Err(_) => failure_count.with_label_values(&[target.clone()]).inc(),
                    },
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => panic!("send disconnected"),
                }
            }
        });
    }
}

/// A dispatcher to send pings (ICMP packets) to a specified target.
struct Dispatcher {
    target: String,
    client: Client,

    result_tx: Sender<Result<()>>,
}

impl Dispatcher {
    fn new(target: String) -> Result<(Self, Receiver<Result<()>>)> {
        let client = surge_ping::Client::new(&Config::new())?;

        let (result_tx, result_rx) = tokio::sync::mpsc::channel(100);
        Ok((
            Self {
                target,
                client,
                result_tx,
            },
            result_rx,
        ))
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
                    self.result_tx.send(Ok(())).await?;
                }
                Err(e) => {
                    eprintln!("{e}");
                    self.result_tx.send(Err(Box::new(e))).await?;
                }
            }
        }
    }
}
