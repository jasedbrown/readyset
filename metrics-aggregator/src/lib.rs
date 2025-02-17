// This design should be re-assessed when we have enough time to do so.
// Relevant ticket: https://readysettech.atlassian.net/browse/ENG-719
#![deny(macro_use_extern_crate)]

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use clap::builder::NonEmptyStringValueParser;
use clap::Parser;
use prometheus_http_query::{Client, Scheme};
use readyset_client::consensus::ConsulAuthority;
use readyset_util::shutdown;
use readyset_version::*;
use tokio::sync::Mutex;
use tracing::info;

use crate::http_router::MetricsAggregatorHttpRouter;

pub mod cache;
pub mod http_router;
pub mod metrics_reconciler;
use cache::QueryMetricsCache;
use metrics_reconciler::MetricsReconciler;

#[derive(Parser)]
#[clap(version = VERSION_STR_PRETTY)]
pub struct Options {
    /// IP:PORT to listen on.
    #[clap(
        long,
        short = 'a',
        env = "LISTEN_ADDRESS",
        default_value = "0.0.0.0:6035"
    )]
    address: SocketAddr,

    /// Consul connection string.
    #[clap(long, short = 'c', env = "AUTHORITY_ADDRESS")]
    consul_address: String,

    /// ReadySet deployment ID to filter by when aggregating metrics.
    #[clap(long, env = "DEPLOYMENT", value_parser = NonEmptyStringValueParser::new())]
    deployment: String,

    /// Prometheus connection string.
    #[clap(long, short = 'p', env = "PROMETHEUS_ADDRESS")]
    prom_address: String,

    /// A flag that if set will communicate with prometheus over http rather than https.
    #[clap(long)]
    unsecured_prometheus: bool,

    /// A flag that if set will communicate with adapters over http rather than https.
    #[clap(long)]
    unsecured_adapters: bool,

    /// A flag that if set will ensure that the metrics aggregator shows latencies from upstream.
    #[clap(long)]
    show_upstream_latencies: bool,

    /// Sets the query reconciler's loop interval in seconds.
    #[clap(long, env = "RECONCILE_INTERVAL", default_value = "20")]
    reconciler_loop_interval: u64,

    #[clap(flatten)]
    tracing: readyset_tracing::Options,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    options
        .tracing
        .init("metrics-aggregator", options.deployment.as_ref())?;
    info!(version = %VERSION_STR_ONELINE);

    let prom_address_res: Vec<SocketAddr> = options.prom_address.to_socket_addrs()?.collect();
    if prom_address_res.is_empty() {
        return Err(anyhow!(
            "supplied prometheus address did not parse correctly"
        ));
    }

    let rt = tokio::runtime::Runtime::new()?;

    let mut sigterm = {
        let _guard = rt.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
    };
    let mut sigint = {
        let _guard = rt.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?
    };

    let (shutdown_tx, shutdown_rx) = shutdown::channel();
    #[allow(clippy::indexing_slicing)] // Validated as safe with early bail above.
    let (prom_ip, prom_port) = (prom_address_res[0].ip(), prom_address_res[0].port());
    let prom_client = if options.unsecured_prometheus {
        Client::new(Scheme::Http, &prom_ip.to_string(), prom_port)
    } else {
        Client::new(Scheme::Https, &prom_ip.to_string(), prom_port)
    };
    let consul = ConsulAuthority::new(&format!(
        "http://{}/{}",
        &options.consul_address, &options.deployment
    ))
    .unwrap();

    let query_metrics_cache = {
        let cache = Arc::new(Mutex::new(QueryMetricsCache::new()));
        let loop_interval = options.reconciler_loop_interval;
        let cache_for_reconciler = cache.clone();
        let deployment = options.deployment.clone();
        let unsecured_adapters = options.unsecured_adapters;
        let show_upstream_latencies = options.show_upstream_latencies;
        let shutdown_rx = shutdown_rx.clone();
        let fut = async move {
            let mut reconciler = MetricsReconciler::new(
                consul,
                prom_client,
                deployment,
                cache_for_reconciler,
                std::time::Duration::from_secs(loop_interval),
                shutdown_rx,
                unsecured_adapters,
                show_upstream_latencies,
            );
            reconciler.run().await
        };

        rt.handle().spawn(fut);

        cache
    };

    // Create the HTTP server for handling metrics aggregator requests.
    let http_server = MetricsAggregatorHttpRouter {
        listen_addr: options.address,
        query_metrics_cache,
    };
    let fut = async move {
        let http_listener = http_server.create_listener().await.unwrap();
        MetricsAggregatorHttpRouter::route_requests(http_server, http_listener, shutdown_rx).await
    };

    rt.handle().spawn(fut);

    // Wait on sigterm or sigint.
    rt.block_on(async move {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("received sigterm");
            },
            _ = sigint.recv() => {
                info!("received sigint");
            },
        }
    });

    info!("shutting down all async threads");

    // We use `shutdown_timeout` instead of `shutdown` in case any blocking IO is ongoing
    info!("Waiting up to 20s for background tasks to complete shutdown");
    rt.block_on(shutdown_tx.shutdown_timeout(Duration::from_secs(20)));

    // TODO(peter): Uncomment once we pull adapter endpoints from authority.
    // // Drop authority channel to close conn with authority.
    // drop(ch);

    // We use `shutdown_timeout` instead of `shutdown_background` in case any
    // blocking IO is ongoing. Most tasks should have been cleaned up from the above shutdown
    // signal, so in practice, this should not take 20 seconds.
    info!("Waiting up to 20s for any lingering tasks to complete shutdown");
    rt.shutdown_timeout(std::time::Duration::from_secs(20));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_parsing() {
        // Certain clap things, like `requires`, only ever throw an error at runtime, not at
        // compile-time - this tests that none of those happen
        let opts = Options::parse_from(vec![
            "metrics-aggregator",
            "--deployment",
            "test",
            "--address",
            "0.0.0.0:8090",
            "-c",
            "0.0.0.0:8500",
            "-p",
            "8.8.8.8:9090",
        ]);

        assert_eq!(opts.deployment, "test");
    }
}
