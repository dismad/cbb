use clap::Parser;
use hdrhistogram::Histogram;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
use tonic::transport::Channel;

pub mod cash {
    pub mod z {
        pub mod wallet {
            pub mod sdk {
                pub mod rpc {
                    tonic::include_proto!("cash.z.wallet.sdk.rpc");
                }
            }
        }
    }
}

use cash::z::wallet::sdk::rpc::{
    compact_tx_streamer_client::CompactTxStreamerClient, BlockId, BlockRange, ChainSpec,
};

#[derive(Parser, Debug)]
#[command(name = "cbb", about = "Compact Block Bench for Zcash lightclient servers")]
struct Args {
    /// Endpoints to test in the form name=url (repeatable)
    #[arg(short, long, value_parser = parse_endpoint)]
    endpoint: Vec<(String, String)>,

    /// Number of concurrent virtual wallets
    #[arg(short, long, default_value_t = 50)]
    concurrency: usize,

    /// Blocks per GetBlockRange request
    #[arg(long, default_value_t = 100)]
    chunk: u64,

    /// How many sequential range requests each wallet makes
    #[arg(long, default_value_t = 20)]
    steps: u64,

    /// Starting height for the first wallet (others are offset)
    #[arg(long, default_value_t = 2_000_000)]
    start_height: u64,

    /// Also call GetTreeState every N range requests (0 = never)
    #[arg(long, default_value_t = 5)]
    tree_every: u64,

    /// Request timeout in seconds, covering the whole block stream
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Warm-up requests per wallet before measuring
    #[arg(long, default_value_t = 2)]
    warmup: u64,

    /// Output JSON summary to stdout
    #[arg(long)]
    json: bool,

    /// Also write JSON summary to this file
    #[arg(long)]
    json_out: Option<String>,

    /// Path to a CA cert PEM to trust (for self-signed zingod)
    #[arg(long)]
    tls_ca: Option<String>,
}

fn parse_endpoint(s: &str) -> Result<(String, String), String> {
    let (name, url) = s.split_once('=').ok_or("expected name=url")?;
    if name.is_empty() || url.is_empty() {
        return Err("name and url must be non-empty".into());
    }
    Ok((name.to_string(), url.to_string()))
}

#[derive(Debug, Clone)]
struct Sample {
    method: &'static str,
    latency_ms: f64,
    ok: bool,
    error: Option<String>,
}

/// One GetBlockRange, read to its last block. Fails on an error status, a missing or
/// out-of-order block, or the timeout.
async fn fetch_range(
    client: &mut CompactTxStreamerClient<Channel>,
    height: u64,
    chunk: u64,
    timeout: Duration,
) -> Result<(), String> {
    let end = height + chunk - 1;
    let req = BlockRange {
        start: Some(BlockId { height, hash: vec![] }),
        end: Some(BlockId { height: end, hash: vec![] }),
    };
    let drain = async {
        let mut stream = client.get_block_range(req).await.map_err(|s| s.to_string())?.into_inner();
        let mut next = height;
        while let Some(block) = stream.message().await.map_err(|s| s.to_string())? {
            if block.height != next {
                return Err("block out of order".to_string());
            }
            next += 1;
        }
        if next != end + 1 {
            return Err("stream ended before the range did".to_string());
        }
        Ok(())
    };
    tokio::time::timeout(timeout, drain).await.unwrap_or_else(|_| Err("timeout".to_string()))
}

async fn run_wallet(
    mut client: CompactTxStreamerClient<Channel>,
    start_height: u64,
    chunk: u64,
    steps: u64,
    tree_every: u64,
    timeout: Duration,
) -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut height = start_height;

    for i in 0..steps {
        let start = Instant::now();
        let result = fetch_range(&mut client, height, chunk, timeout).await;
        samples.push(Sample {
            method: "GetBlockRange",
            latency_ms: start.elapsed().as_secs_f64() * 1000.0,
            ok: result.is_ok(),
            error: result.err(),
        });

        if tree_every > 0 && i % tree_every == 0 {
            let id = BlockId { height, hash: vec![] };
            let start = Instant::now();
            let result = tokio::time::timeout(timeout, client.get_tree_state(id)).await;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            samples.push(Sample {
                method: "GetTreeState",
                latency_ms: elapsed,
                ok: result.is_ok() && result.as_ref().unwrap().is_ok(),
                error: match &result {
                    Ok(Ok(_)) => None,
                    Ok(Err(status)) => Some(status.to_string()),
                    Err(_) => Some("timeout".into()),
                },
            });
        }

        height += chunk;
    }

    samples
}

fn summarize(name: &str, samples: &[Sample]) -> Summary {
    let mut hist = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
    let mut errors = 0u64;
    let mut error_msgs: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut by_method: std::collections::HashMap<&str, Vec<f64>> = std::collections::HashMap::new();
    let mut method_errors: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();

    for s in samples {
        if s.ok {
            let _ = hist.record((s.latency_ms * 1000.0) as u64);
            by_method.entry(s.method).or_default().push(s.latency_ms);
        } else {
            errors += 1;
            *method_errors.entry(s.method).or_default() += 1;
            if let Some(msg) = &s.error {
                *error_msgs.entry(msg.clone()).or_default() += 1;
            }
        }
    }

    let total = samples.len() as u64;
    Summary {
        name: name.to_string(),
        total_requests: total,
        errors,
        error_rate: if total == 0 { 0.0 } else { errors as f64 / total as f64 },
        p50: hist.value_at_quantile(0.50) as f64 / 1000.0,
        p95: hist.value_at_quantile(0.95) as f64 / 1000.0,
        p99: hist.value_at_quantile(0.99) as f64 / 1000.0,
        mean: hist.mean() / 1000.0,
        max: hist.max() as f64 / 1000.0,
        wall_time_secs: 0.0,
        throughput_rps: 0.0,
        methods: by_method
            .into_iter()
            .map(|(m, v)| {
                let mut h = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
                for x in &v {
                    let _ = h.record((*x * 1000.0) as u64);
                }
                MethodSummary {
                    method: m.to_string(),
                    count: v.len() as u64,
                    errors: *method_errors.get(m).unwrap_or(&0),
                    p50: h.value_at_quantile(0.50) as f64 / 1000.0,
                    p95: h.value_at_quantile(0.95) as f64 / 1000.0,
                    p99: h.value_at_quantile(0.99) as f64 / 1000.0,
                    max: h.max() as f64 / 1000.0,
                }
            })
            .collect(),
        error_samples: error_msgs.into_iter().collect(),
    }
}

#[derive(Debug, serde::Serialize)]
struct MethodSummary {
    method: String,
    count: u64,
    errors: u64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

#[derive(Debug, serde::Serialize)]
struct Summary {
    name: String,
    total_requests: u64,
    errors: u64,
    error_rate: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    mean: f64,
    max: f64,
    wall_time_secs: f64,
    throughput_rps: f64,
    methods: Vec<MethodSummary>,
    error_samples: Vec<(String, u64)>,
}

fn print_summary(s: &Summary) {
    println!("── {} ──", s.name);
    println!(
        "  requests: {}   errors: {} ({:.1}%)",
        s.total_requests, s.errors, s.error_rate * 100.0
    );
    println!();
    println!("  {:<16} {:>8} {:>8} {:>10} {:>10} {:>10} {:>10}",
             "method", "count", "errors", "p50", "p95", "p99", "max");
    println!("  {:<16} {:>8} {:>8} {:>10} {:>10} {:>10} {:>10}",
             "------", "-----", "------", "---", "---", "---", "---");

    println!("  {:<16} {:>8} {:>8} {:>9.1}ms {:>9.1}ms {:>9.1}ms {:>9.1}ms",
             "overall", s.total_requests, s.errors, s.p50, s.p95, s.p99, s.max);

    for m in &s.methods {
        println!("  {:<16} {:>8} {:>8} {:>9.1}ms {:>9.1}ms {:>9.1}ms {:>9.1}ms",
                 m.method, m.count, m.errors, m.p50, m.p95, m.p99, m.max);
    }
    println!();
    if !s.error_samples.is_empty() {
        println!("  errors:");
        let mut errs = s.error_samples.clone();
        errs.sort_by(|a, b| b.1.cmp(&a.1));
        for (msg, count) in errs.iter().take(5) {
            println!("    {}×  {}", count, msg);
        }
        println!();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.endpoint.is_empty() {
        eprintln!("error: at least one --endpoint name=url is required");
        eprintln!("example: --endpoint lwd=http://127.0.0.1:9067 --endpoint zainod=http://127.0.0.1:8137");
        std::process::exit(1);
    }

    let timeout = Duration::from_secs(args.timeout);
    let mut all_summaries = Vec::new();

    for (name, url) in &args.endpoint {
        eprintln!("Connecting to {} ({})...", name, url);

        // build the endpoint template (TLS config applied per connection below)
        let mut endpoint = Channel::from_shared(url.clone()).unwrap().timeout(timeout);
        if let Some(ca_path) = &args.tls_ca {
            if url.starts_with("https") {
                let pem = std::fs::read(ca_path)
                    .unwrap_or_else(|e| panic!("failed to read {}: {}", ca_path, e));
                let ca = tonic::transport::Certificate::from_pem(pem);
                let tls = tonic::transport::ClientTlsConfig::new()
                    .ca_certificate(ca)
                    .domain_name("localhost");
                endpoint = endpoint.tls_config(tls).expect("tls config");
            }
        }

        // quick health check
        match endpoint.connect().await {
            Ok(ch) => {
                let mut probe = CompactTxStreamerClient::new(ch);
                match tokio::time::timeout(timeout, probe.get_lightd_info(ChainSpec {})).await {
                    Ok(Ok(info)) => {
                        let i = info.into_inner();
                        eprintln!("  vendor={} chain={} height={}", i.vendor, i.chain_name, i.block_height);
                    }
                    _ => println!("  (GetLightdInfo failed or timed out, continuing anyway)"),
                }
            }
            Err(e) => {
                eprintln!("  failed to connect: {}", e);
                continue;
            }
        }

        let mut tasks = Vec::new();
        let chunk = args.chunk;
        let steps = args.steps;
        let tree_every = args.tree_every;
        let warmup = args.warmup;
        // the clock starts once every wallet (and this task) has connected and warmed up
        let start_line = Arc::new(Barrier::new(args.concurrency + 1));

        for w in 0..args.concurrency {
            let start = args.start_height + (w as u64 * 10);
            // clone the configured endpoint so each wallet gets its own connection
            let ep = endpoint.clone();
            let start_line = Arc::clone(&start_line);
            tasks.push(tokio::spawn(async move {
                let channel = match ep.connect().await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("wallet connect failed: {}", e);
                        start_line.wait().await;
                        return Vec::new();
                    }
                };
                let mut client = CompactTxStreamerClient::new(channel);
                // warm-up (not recorded)
                for i in 0..warmup {
                    let _ = fetch_range(&mut client, start + i * chunk, chunk, timeout).await;
                }
                start_line.wait().await;
                run_wallet(client, start + warmup * chunk, chunk, steps, tree_every, timeout).await
            }));
        }

        eprintln!("  running {} wallets × {} steps (one connection each)...", args.concurrency, args.steps);
        start_line.wait().await;
        let wall_start = Instant::now();
        let results = futures::future::join_all(tasks).await;
        let wall_elapsed = wall_start.elapsed();
        let samples: Vec<Sample> = results.into_iter().filter_map(|r| r.ok()).flatten().collect();

        let mut summary = summarize(name, &samples);
        let wall_secs = wall_elapsed.as_secs_f64();
        let rps = if wall_secs > 0.0 { samples.len() as f64 / wall_secs } else { 0.0 };
        summary.wall_time_secs = wall_secs;
        summary.throughput_rps = rps;
        if !args.json {
            print_summary(&summary);
            println!("  wall time: {:.2}s   throughput: {:.0} req/s", wall_secs, rps);
            println!();
        }
        all_summaries.push(summary);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&all_summaries)?);
    }
    if let Some(path) = &args.json_out {
        std::fs::write(path, serde_json::to_string_pretty(&all_summaries)?)?;
        eprintln!("wrote {}", path);
    }

    Ok(())
}
