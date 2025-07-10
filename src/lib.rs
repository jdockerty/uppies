use std::{net::IpAddr, str::FromStr, time::Duration};

use prometheus::{IntCounterVec, Opts, Registry};
use surge_ping::{Client, Config, PingIdentifier, PingSequence};
use tokio::sync::mpsc::{error::TryRecvError, Receiver, Sender};
use tracing::{debug, error, info};

pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

/// Send pings to various targets.
pub struct PingSender {
    /// Dispatchers send pings to the underlying targets.
    ///
    /// The corresponding [`Receiver`] returns the result dependent on the outcome
    /// of the pin.g
    dispatchers: Vec<(Dispatcher, Receiver<Result<()>>)>,

    /// Number of pings which were successful, labelled by the underlying target.
    success_count: IntCounterVec,
    /// Number of pings which were unsuccessful, labelled by the underlying target.
    failure_count: IntCounterVec,
}

impl PingSender {
    const LABELS: &[&str] = &["targets"];

    pub fn new(targets: Vec<String>, ping_interval_ms: u64, metrics: &Registry) -> Result<Self> {
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
                .map(|t| Dispatcher::new(t.clone(), ping_interval_ms))
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

        // Check the receive channel 2x faster than the known ping interval
        // to ensure that all sends are caught in good time.
        let receive_interval = dispatcher.ping_interval_ms.div_ceil(2);
        let target = dispatcher.target.clone();
        info!(target, "starting dispatcher tasks");
        tokio::spawn(dispatcher.run());
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(receive_interval));
            loop {
                interval.tick().await;
                match rx.try_recv() {
                    Ok(res) => match res {
                        Ok(_) => success_count.with_label_values(&[target.clone()]).inc(),
                        Err(_) => failure_count.with_label_values(&[target.clone()]).inc(),
                    },
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => panic!("send disconnected"),
                }
            }
        });
    }
}

/// A dispatcher to send pings (ICMP packets) to a specified target.
struct Dispatcher {
    /// The underlying target of this [`Dispatcher`], such as
    /// '1.1.1.1'.
    target: String,
    /// Internal client used to send ICMP packets.
    client: Client,
    /// Result channel for receiving dispatched ping results.
    result_tx: Sender<Result<()>>,

    ping_interval_ms: u64,
}

impl Dispatcher {
    /// Create a new [`Dispatcher`] with an accompanying [`Receiver`] that
    /// will be used to send ping results into.
    fn new(target: String, ping_interval_ms: u64) -> Result<(Self, Receiver<Result<()>>)> {
        let client = surge_ping::Client::new(&Config::new())?;

        let (result_tx, result_rx) = tokio::sync::mpsc::channel(5);
        Ok((
            Self {
                target,
                client,
                result_tx,
                ping_interval_ms,
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

        let mut interval = tokio::time::interval(Duration::from_millis(self.ping_interval_ms));
        loop {
            interval.tick().await;
            match pinger.ping(PingSequence(0), &[]).await {
                Ok((_, duration)) => {
                    debug!(target = self.target, ?duration, "ping success");
                    self.result_tx.send(Ok(())).await?;
                }
                Err(e) => {
                    error!(target = self.target, ?e, "ping failure");
                    self.result_tx.send(Err(Box::new(e))).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use crate::Dispatcher;

    const LOCALHOST: &str = "127.0.0.1";
    const TEST_DURATION_MS: u64 = 200;

    #[tokio::test]
    async fn dispatcher_success() {
        let (dispatcher, mut rx) =
            Dispatcher::new(LOCALHOST.to_string(), TEST_DURATION_MS).unwrap();
        tokio::spawn(dispatcher.run());

        let res = tokio::time::timeout(Duration::from_millis(TEST_DURATION_MS * 3), async move {
            loop {
                match rx.recv().await {
                    Some(res) => return res,
                    None => continue,
                }
            }
        })
        .await
        .expect("no success received");

        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn dispatcher_failure() {
        let unbound_addr = "10.0.0.200"; // this could be flakey
        let (dispatcher, mut rx) =
            Dispatcher::new(unbound_addr.to_string(), TEST_DURATION_MS).unwrap();
        tokio::spawn(dispatcher.run());

        // Use an increased timeout, the default in the client is 2s before
        // an error will be served back.
        // TODO: create `Dispatcher::with_pinger`?
        let res = tokio::time::timeout(Duration::from_secs(5), async move {
            loop {
                match rx.recv().await {
                    Some(res) => return res,
                    None => continue,
                }
            }
        })
        .await
        .expect("no success received");

        assert!(res.is_err());
    }
}
