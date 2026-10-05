mod balance_monitor;
mod config;

use anyhow::Result;
use clap::Parser;
use prometheus::Encoder as _;
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use web3::types::U256;

#[derive(Debug, Parser)]
struct Opt {
    /// Path to the config file.
    #[clap(long, parse(from_os_str))]
    config: PathBuf,

    /// Serve the prometheus metrics at this address.
    #[clap(long, default_value = "0.0.0.0:8080")]
    bind: SocketAddr,

    /// Update the balances in this interval in seconds.
    #[clap(long, default_value = "100", parse(try_from_str = duration_from_seconds))]
    update_interval: Duration,

    /// Print balances to stdout on update.
    #[clap(long)]
    print_balances: bool,
}

fn duration_from_seconds(s: &str) -> Result<Duration, std::num::ParseIntError> {
    s.parse().map(Duration::from_secs)
}

fn print_balance(address_name: &str, network_name: &str, token_name: &str, balance: &Result<U256>) {
    match balance {
        Ok(balance) => println!(
            "address {} on network {} token {} balance is {}",
            address_name, network_name, token_name, balance
        ),
        Err(err) => println!(
            "failed to get balance for address {} on network {} token {}: {}",
            address_name, network_name, token_name, err
        ),
    }
}

// Copied from ethcontract-rs.
/// Lossy conversion from a `U256` to a `f64`.
pub fn u256_to_f64(value: U256) -> f64 {
    // NOTE: IEEE 754 double precision floats (AKA `f64`) have 53 bits of
    //   precision, take 1 extra bit so that the `u64` to `f64` conversion does
    //   rounding for us, instead of implementing it ourselves.
    let exponent = value.bits().saturating_sub(54);
    let mantissa = (value >> U256::from(exponent)).as_u64();

    (mantissa as f64) * 2.0f64.powi(exponent as i32)
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::parse();
    println!("Beginning service with configuration parameters {:#?}", opt);
    let config: config::Config = toml::from_str(&std::fs::read_to_string(opt.config)?)?;
    println!("Monitoring accounts {:#?}", config);

    let monitor = balance_monitor::BalanceMonitor::new(config)?;

    // metrics
    let balance_metric = prometheus::GaugeVec::new(
        prometheus::Opts::new(
            "etherbalance_balance",
            "The native (ether, SOL) or IERC20 balance of an address.",
        ),
        &["address_name", "token_name", "address", "tag", "network"],
    )?;
    let success_metric = prometheus::IntCounterVec::new(
        prometheus::Opts::new("success_counter", "Success/Failure counts"),
        &["result", "address", "network"],
    )?;
    let last_update_metric = prometheus::Gauge::new(
        "etherbalance_last_update",
        "Unix time of last update of balances.",
    )?;
    let registry = prometheus::Registry::new();
    registry.register(Box::new(balance_metric.clone()))?;
    registry.register(Box::new(success_metric.clone()))?;
    registry.register(Box::new(last_update_metric.clone()))?;

    // http server for metrics
    let address = opt.bind;
    std::thread::spawn(move || {
        let encoder = prometheus::TextEncoder::new();
        rouille::start_server(address, move |_request| {
            // We always serve the the metrics regardless of path even though
            // the readme states the path should be /metrics.
            let metric_families = registry.gather();
            let mut buffer = vec![];
            encoder
                .encode(&metric_families, &mut buffer)
                .expect("could not encode metrics");
            rouille::Response::from_data("text/plain; charset=utf-8", buffer)
        });
    });

    // update balances
    let print_balances = opt.print_balances;
    loop {
        monitor
            .do_with_balances(|params| {
                if print_balances {
                    print_balance(
                        params.address_name,
                        params.network_name,
                        params.token_name,
                        &params.balance,
                    );
                }
                match params.balance {
                    Ok(balance) => {
                        balance_metric
                            .with_label_values(&[
                                params.address_name,
                                params.token_name,
                                &params.address.label(),
                                params.tag,
                                params.network_name,
                            ])
                            .set(u256_to_f64(balance));
                        success_metric
                            .with_label_values(&[
                                "success",
                                &params.address.label(),
                                params.network_name,
                            ])
                            .inc();
                    }
                    Err(err) => {
                        success_metric
                            .with_label_values(&[
                                "failure",
                                &params.address.label(),
                                params.network_name,
                            ])
                            .inc();
                        println!(
                            "failed to get balance for address {} token {}: {}",
                            params.address.label(),
                            params.token_name,
                            err
                        );
                    }
                }
            })
            .await;
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => last_update_metric.set(duration.as_secs_f64()),
            Err(err) => println!("system time before epoch: {}", err),
        };
        // Retrieving the balances takes some time so sleeping for
        // update_interval makes us actually update the balances less frequently
        // than update_interval. We could be more accurate and sleep the exact
        // time needed. In practice it does not matter.
        tokio::time::sleep(opt.update_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_parse_example_config() {
        let config = include_str!("../example_config.toml");
        let _: config::Config = toml::from_str(config).unwrap();
    }
}
