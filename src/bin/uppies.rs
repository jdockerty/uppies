use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, Response},
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;

use prometheus::{Encoder, Registry, TextEncoder};
use tokio::net::TcpListener;
use uppies::{ping_targets, PingSender, Result};

#[derive(Debug, Parser)]
struct Cli {
    /// Targets that should have pings sent to them.
    targets: Vec<String>,

    /// Socket to bind to serve metrics.
    #[clap(long, default_value = "0.0.0.0:9000")]
    metrics_address: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let metrics = Registry::default();

    let sender = PingSender::new(cli.targets, &metrics)?;
    ping_targets(sender).await;

    let metric_listener = TcpListener::bind(&cli.metrics_address).await?;
    tokio::spawn(async move {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(AppState { metrics });
        axum::serve(metric_listener, app).await.unwrap();
    });

    tokio::signal::ctrl_c().await?;

    Ok(())
}

#[derive(Clone)]
struct AppState {
    metrics: Registry,
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let text_encoder = TextEncoder::new();
    let metric_family = state.metrics.gather();

    Response::builder()
        .header(CONTENT_TYPE, text_encoder.format_type())
        .body(
            text_encoder
                .encode_to_string(&metric_family)
                .expect("can encode known metrics"),
        )
        .expect("valid response type")
}
