use std::{net::IpAddr, str::FromStr, time::Duration};

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};
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
    dispatchers: Vec<(Dispatcher, Receiver<Result<Duration>>)>,

    /// Number of pings which were successful, labelled by the underlying target.
    success_count: IntCounterVec,
    /// Number of pings which were unsuccessful, labelled by the underlying target.
    failure_count: IntCounterVec,

    /// Histogram of ping durations in milliseconds, labelled by the underlying target.
    ping_duration_ms: HistogramVec,
}

impl PingSender {
    const LABELS: &[&str] = &["target"];

    pub fn new(targets: Vec<String>, ping_interval_ms: u64, metrics: &Registry) -> Result<Self> {
        let success_count = IntCounterVec::new(
            Opts::new("ping_success_count", "Counter of successful pings"),
            Self::LABELS,
        )?;
        let failure_count = IntCounterVec::new(
            Opts::new("ping_failure_count", "Counter of failed pings"),
            Self::LABELS,
        )?;
        let ping_duration_ms = HistogramVec::new(
            HistogramOpts::new(
                "ping_duration_ms",
                "Histogram of ping round-trip times in milliseconds",
            )
            .buckets(vec![
                1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0,
            ]),
            Self::LABELS,
        )?;
        metrics.register(Box::new(success_count.clone()))?;
        metrics.register(Box::new(failure_count.clone()))?;
        metrics.register(Box::new(ping_duration_ms.clone()))?;
        Ok(Self {
            dispatchers: targets
                .iter()
                .map(|t| Dispatcher::new(t.clone(), ping_interval_ms))
                .collect::<Result<_>>()?,
            success_count,
            failure_count,
            ping_duration_ms,
        })
    }
}

/// Start pinging all targets configured within the [`PingSender`]
pub async fn ping_targets(sender: PingSender) {
    for (dispatcher, mut rx) in sender.dispatchers {
        let success_count = sender.success_count.clone();
        let failure_count = sender.failure_count.clone();
        let ping_duration_ms = sender.ping_duration_ms.clone();

        // Check the receive channel 2x faster than the known ping interval
        // to ensure that all sends are caught in good time.
        let receive_interval = dispatcher.ping_interval_ms.div_ceil(2);
        let target = dispatcher.target.clone();
        info!(target, "starting dispatcher tasks");
        // Initialise the value on start, this allows the
        // metric to be immediately reported as 0 if there are no
        // errors for sometime.
        failure_count.with_label_values(&[target.clone()]).inc_by(0);
        tokio::spawn(dispatcher.run(None));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(receive_interval));
            loop {
                interval.tick().await;
                match rx.try_recv() {
                    Ok(res) => match res {
                        Ok(d) => {
                            success_count.with_label_values(&[target.clone()]).inc();
                            ping_duration_ms
                                .with_label_values(&[target.clone()])
                                .observe(d.as_millis() as f64);
                        }
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
    result_tx: Sender<Result<Duration>>,

    ping_interval_ms: u64,
}

impl Dispatcher {
    /// Create a new [`Dispatcher`] with an accompanying [`Receiver`] that
    /// will be used to send ping results into.
    fn new(target: String, ping_interval_ms: u64) -> Result<(Self, Receiver<Result<Duration>>)> {
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
    /// A `timeout` can be provided, which alters the length of time before
    /// a timeout error is issued for the dispatched ping against a target.
    ///
    /// This is a blocking call and will perform continuous pings against
    /// the target.
    async fn run(self, timeout: Option<Duration>) -> Result<()> {
        let mut pinger = self
            .client
            .pinger(
                IpAddr::from_str(&self.target)?,
                PingIdentifier(rand::random()),
            )
            .await;

        if let Some(timeout) = timeout {
            pinger.timeout(timeout);
        }

        let mut interval = tokio::time::interval(Duration::from_millis(self.ping_interval_ms));
        loop {
            interval.tick().await;
            match pinger.ping(PingSequence(0), &[]).await {
                Ok((_, duration)) => {
                    debug!(target = self.target, ?duration, "ping success");
                    self.result_tx.send(Ok(duration)).await?;
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

    use prometheus::{
        core::{Atomic, GenericCounterVec},
        Registry,
    };

    use crate::{ping_targets, Dispatcher, PingSender};

    const LOCALHOST: &str = "127.0.0.1";
    const TEST_DURATION_MS: u64 = 200;

    #[tokio::test]
    async fn dispatcher_success() {
        let (dispatcher, mut rx) =
            Dispatcher::new(LOCALHOST.to_string(), TEST_DURATION_MS).unwrap();
        tokio::spawn(dispatcher.run(None));

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
        tokio::spawn(dispatcher.run(Some(Duration::from_millis(100)))); // short time-out duration

        let res = tokio::time::timeout(Duration::from_secs(1), async move {
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

    fn get_metric_value<P: Atomic>(metric_value: GenericCounterVec<P>, target: &str) -> P::T {
        metric_value
            .get_metric_with_label_values(&[target])
            .unwrap()
            .get()
    }

    #[tokio::test]
    async fn pings() {
        let metrics = Registry::new();
        let ping_sender = PingSender::new(
            [LOCALHOST, LOCALHOST]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            TEST_DURATION_MS,
            &metrics,
        )
        .unwrap();

        let success_count = ping_sender.success_count.clone();
        let failure_count = ping_sender.failure_count.clone();
        let ping_duration_histogram = ping_sender.ping_duration_ms.clone();

        tokio::spawn(ping_targets(ping_sender));

        // Let the sender run in the background before asserting
        tokio::time::sleep(Duration::from_secs(1)).await;

        assert!(
            get_metric_value(success_count, LOCALHOST) > 0,
            "Success counter should have increased"
        );
        assert_eq!(
            get_metric_value(failure_count, LOCALHOST),
            0,
            "Failure counter should still be 0"
        );
        assert!(
            ping_duration_histogram
                .with_label_values(&[LOCALHOST])
                .get_sample_count()
                > 0
        );
    }
}
