use clap::Parser;
use hdrhistogram::Histogram;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
use tonic::transport::{Channel, Endpoint};

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
    compact_tx_streamer_client::CompactTxStreamerClient, BlockId, BlockRange, ChainSpec, TxFilter,
};

/// Blocks downloaded from --start-height to collect transaction IDs for --tx.
const TX_SOURCE_BLOCKS: u64 = 1_000;
/// Blocks per GetBlockRange while collecting transaction IDs.
const TX_SOURCE_BATCH: u64 = 100;

/// A mined transaction to request: its ID as sent in CompactTx.hash and the
/// height of the block it came from.
#[derive(Debug, Clone)]
struct TxRef {
    hash: Vec<u8>,
    height: u64,
}

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

    /// Measured range, tree-only or tx requests per wallet
    #[arg(long, default_value_t = 20)]
    steps: u64,

    /// Starting height for the first wallet (others are offset); with --tx, where transaction IDs are collected
    #[arg(long, default_value_t = 2_000_000)]
    start_height: u64,

    /// Also call GetTreeState every N range requests (0 = never)
    #[arg(long, default_value_t = 5)]
    tree_every: u64,

    /// Run only GetTreeState with one unsigned decimal height per line
    #[arg(long, value_name = "PATH")]
    tree_heights_file: Option<PathBuf>,

    /// Runs GetTransaction only; skips GetBlockRange and GetTreeState
    #[arg(long, conflicts_with = "tree_heights_file")]
    tx: bool,

    /// Connection and request timeout in seconds, including the whole block stream
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

fn rpc_error(status: tonic::Status) -> String {
    format!("{:?}: {}", status.code(), status.message())
}

fn read_tree_heights(path: &Path) -> std::io::Result<Arc<[u64]>> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
    })?;
    let mut heights = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let invalid = |message: &str| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}:{}: {message}", path.display(), index + 1),
            )
        };
        let height = line
            .parse::<u64>()
            .ok()
            .filter(|_| line.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| invalid("expected an unsigned decimal height"))?;
        if !seen.insert(height) {
            return Err(invalid("duplicate height"));
        }
        heights.push(height);
    }
    if heights.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: expected at least one height", path.display()),
        ));
    }
    Ok(heights.into())
}

#[derive(Debug, Clone)]
struct Sample {
    method: &'static str,
    latency_ms: f64,
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
        let mut stream = client.get_block_range(req).await.map_err(rpc_error)?.into_inner();
        let mut next = height;
        while let Some(block) = stream.message().await.map_err(rpc_error)? {
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
            error: result.err(),
        });

        if tree_every > 0 && i % tree_every == 0 {
            let start = Instant::now();
            let result = fetch_tree_height(&mut client, height, timeout).await;
            samples.push(Sample {
                method: "GetTreeState",
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                error: result.err(),
            });
        }

        height += chunk;
    }

    samples
}

async fn fetch_tree_height(
    client: &mut CompactTxStreamerClient<Channel>,
    height: u64,
    timeout: Duration,
) -> Result<u64, String> {
    let id = BlockId { height, hash: vec![] };
    tokio::time::timeout(timeout, client.get_tree_state(id))
        .await
        .map_err(|_| "timeout".to_string())?
        .map(|response| response.into_inner().height)
        .map_err(rpc_error)
}

async fn run_tree_wallet(
    mut client: CompactTxStreamerClient<Channel>,
    heights: Arc<[u64]>,
    mut index: usize,
    steps: u64,
    timeout: Duration,
) -> Vec<Sample> {
    let mut samples = Vec::new();
    for _ in 0..steps {
        let height = heights[index];
        let start = Instant::now();
        let result = fetch_tree_height(&mut client, height, timeout).await;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let error = match result {
            Ok(actual) if actual != height => {
                Some(format!("GetTreeState returned height {actual}, expected {height}"))
            }
            result => result.err(),
        };
        samples.push(Sample { method: "GetTreeState", latency_ms: elapsed, error });
        index = (index + 1) % heights.len();
    }
    samples
}

/// Downloads TX_SOURCE_BLOCKS compact blocks from `start_height` and returns
/// every transaction listed in them, with the height of its block. Not timed.
async fn collect_txs(
    client: &mut CompactTxStreamerClient<Channel>,
    start_height: u64,
    timeout: Duration,
) -> Result<Arc<[TxRef]>, String> {
    let end_height = start_height + TX_SOURCE_BLOCKS - 1;
    let mut txs = Vec::new();
    let mut height = start_height;
    while height <= end_height {
        let end = (height + TX_SOURCE_BATCH - 1).min(end_height);
        let req = BlockRange {
            start: Some(BlockId { height, hash: vec![] }),
            end: Some(BlockId { height: end, hash: vec![] }),
        };
        let drain = async {
            let mut stream = client.get_block_range(req).await.map_err(rpc_error)?.into_inner();
            let mut next = height;
            while let Some(block) = stream.message().await.map_err(rpc_error)? {
                if block.height != next {
                    return Err("block out of order".to_string());
                }
                txs.extend(block.vtx.into_iter().map(|tx| TxRef { hash: tx.hash, height: next }));
                next += 1;
            }
            if next != end + 1 {
                return Err("stream ended before the range did".to_string());
            }
            Ok(())
        };
        tokio::time::timeout(timeout, drain)
            .await
            .unwrap_or_else(|_| Err("timeout".to_string()))
            .map_err(|error| format!("blocks {height}-{end}: {error}"))?;
        height = end + 1;
    }
    if txs.is_empty() {
        return Err(format!("no transactions in compact blocks {start_height}-{end_height}"));
    }
    Ok(txs.into())
}

/// One GetTransaction. Fails on an error status, the timeout, an empty reply,
/// or a mined height other than the block the ID came from.
async fn fetch_tx(
    client: &mut CompactTxStreamerClient<Channel>,
    tx: &TxRef,
    timeout: Duration,
) -> Result<(), String> {
    let req = TxFilter { block: None, index: 0, hash: tx.hash.clone() };
    let reply = tokio::time::timeout(timeout, client.get_transaction(req))
        .await
        .map_err(|_| "timeout".to_string())?
        .map_err(rpc_error)?
        .into_inner();
    if reply.data.is_empty() {
        return Err("GetTransaction returned no transaction data".to_string());
    }
    if reply.height != tx.height {
        return Err(format!(
            "GetTransaction returned height {}, expected {}",
            reply.height, tx.height
        ));
    }
    Ok(())
}

async fn run_tx_wallet(
    mut client: CompactTxStreamerClient<Channel>,
    txs: Arc<[TxRef]>,
    mut index: usize,
    steps: u64,
    timeout: Duration,
) -> Vec<Sample> {
    let mut samples = Vec::new();
    for _ in 0..steps {
        let start = Instant::now();
        let result = fetch_tx(&mut client, &txs[index], timeout).await;
        samples.push(Sample {
            method: "GetTransaction",
            latency_ms: start.elapsed().as_secs_f64() * 1000.0,
            error: result.err(),
        });
        index = (index + 1) % txs.len();
    }
    samples
}

fn summarize(name: &str, samples: &[Sample], wall_secs: f64, status: RunStatus) -> Summary {
    let mut hist = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
    let mut errors = 0;
    let mut error_msgs = HashMap::new();
    let mut by_method: HashMap<&str, (Vec<f64>, u64)> = HashMap::new();

    for sample in samples {
        let (latencies, method_errors) = by_method.entry(sample.method).or_default();
        if let Some(message) = &sample.error {
            errors += 1;
            *method_errors += 1;
            *error_msgs.entry(message.clone()).or_default() += 1;
        } else {
            let _ = hist.record((sample.latency_ms * 1000.0) as u64);
            latencies.push(sample.latency_ms);
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
        wall_time_secs: wall_secs,
        throughput_rps: if wall_secs > 0.0 { (total - errors) as f64 / wall_secs } else { 0.0 },
        methods: by_method
            .into_iter()
            .map(|(method, (latencies, errors))| {
                let mut h = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap();
                for x in &latencies {
                    let _ = h.record((*x * 1000.0) as u64);
                }
                MethodSummary {
                    method: method.to_string(),
                    count: latencies.len() as u64,
                    errors,
                    p50: h.value_at_quantile(0.50) as f64 / 1000.0,
                    p95: h.value_at_quantile(0.95) as f64 / 1000.0,
                    p99: h.value_at_quantile(0.99) as f64 / 1000.0,
                    max: h.max() as f64 / 1000.0,
                }
            })
            .collect(),
        error_samples: error_msgs.into_iter().collect(),
        status,
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
struct RunStatus {
    wallets_configured: usize,
    wallets_connected: usize,
    wallet_connection_errors: usize,
    wallet_task_errors: usize,
    probe_error: Option<String>,
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
    #[serde(flatten)]
    status: RunStatus,
}

fn print_summary(s: &Summary) {
    println!("── {} ──", s.name);
    println!(
        "  wallets connected: {}/{}   connection errors: {}   wallet task errors: {}",
        s.status.wallets_connected,
        s.status.wallets_configured,
        s.status.wallet_connection_errors,
        s.status.wallet_task_errors,
    );
    if let Some(error) = &s.status.probe_error {
        println!("  probe error: {error}");
    }
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

async fn connect(
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<CompactTxStreamerClient<Channel>, String> {
    tokio::time::timeout(timeout, endpoint.connect())
        .await
        .map_err(|_| "connection timed out".to_owned())?
        .map(CompactTxStreamerClient::new)
        .map_err(|error| error.to_string())
}

async fn probe(endpoint: &Endpoint, timeout: Duration) -> Result<(), String> {
    let mut client = connect(endpoint, timeout)
        .await
        .map_err(|error| format!("probe connection failed: {error}"))?;
    let response = tokio::time::timeout(timeout, client.get_lightd_info(ChainSpec {}))
        .await
        .map_err(|_| "GetLightdInfo timed out".to_owned())?
        .map_err(|error| format!("GetLightdInfo failed: {}", rpc_error(error)))?;
    let info = response.into_inner();
    eprintln!("  vendor={} chain={} height={}", info.vendor, info.chain_name, info.block_height);
    Ok(())
}

enum WalletWorkload {
    Ranges(u64),
    Trees { heights: Arc<[u64]>, index: usize },
    Txs { txs: Arc<[TxRef]>, index: usize },
}

async fn run_endpoint(
    name: &str,
    endpoint: Endpoint,
    args: &Args,
    tree_heights: Option<Arc<[u64]>>,
    timeout: Duration,
) -> Result<Summary, String> {
    let probe_error = if tree_heights.is_some() {
        None
    } else {
        probe(&endpoint, timeout).await.err()
    };
    if let Some(error) = &probe_error {
        eprintln!("  {error}");
    }

    let txs = if args.tx {
        let collected = match connect(&endpoint, timeout).await {
            Ok(mut client) => collect_txs(&mut client, args.start_height, timeout).await,
            Err(error) => Err(format!("connection failed: {error}")),
        };
        match collected {
            Ok(txs) => {
                eprintln!(
                    "  collected {} transactions from blocks {}-{}",
                    txs.len(),
                    args.start_height,
                    args.start_height + TX_SOURCE_BLOCKS - 1,
                );
                Some(txs)
            }
            Err(error) => {
                let error = format!("transaction collection failed: {error}");
                eprintln!("  {error}");
                return Err(error);
            }
        }
    } else {
        None
    };

    let chunk = args.chunk;
    let steps = args.steps;
    let tree_every = args.tree_every;
    let warmup = args.warmup;
    let start_height = args.start_height;
    let setup = (0..args.concurrency).map(|wallet| {
        let endpoint = endpoint.clone();
        let tree_heights = tree_heights.clone();
        let txs = txs.clone();
        tokio::spawn(async move {
            let mut client = connect(&endpoint, timeout).await?;
            let workload = if let Some(heights) = tree_heights {
                let index = (wallet as u128 * u128::from(steps) % heights.len() as u128) as usize;
                for _ in 0..warmup {
                    let _ = fetch_tree_height(&mut client, heights[index], timeout).await;
                }
                WalletWorkload::Trees { heights, index }
            } else if let Some(txs) = txs {
                // Each wallet takes its own run of warmup + steps transactions,
                // warming up on the first ones and timing the rest, as the range
                // workload warms up on the blocks before the ones it times.
                let len = txs.len() as u128;
                let first = wallet as u128 * (u128::from(warmup) + u128::from(steps)) % len;
                for step in 0..u128::from(warmup) {
                    let at = ((first + step) % len) as usize;
                    let _ = fetch_tx(&mut client, &txs[at], timeout).await;
                }
                let index = ((first + u128::from(warmup)) % len) as usize;
                WalletWorkload::Txs { txs, index }
            } else {
                let start = start_height + wallet as u64 * 10;
                for step in 0..warmup {
                    let _ = fetch_range(&mut client, start + step * chunk, chunk, timeout).await;
                }
                WalletWorkload::Ranges(start + warmup * chunk)
            };
            Ok::<_, String>((client, workload))
        })
    });

    let mut status = RunStatus {
        wallets_configured: args.concurrency,
        wallets_connected: 0,
        wallet_connection_errors: 0,
        wallet_task_errors: 0,
        probe_error,
    };
    let mut ready = Vec::new();
    for result in futures::future::join_all(setup).await {
        match result {
            Ok(Ok(wallet)) => ready.push(wallet),
            Ok(Err(error)) => {
                status.wallet_connection_errors += 1;
                eprintln!("  wallet connection failed: {error}");
            }
            Err(error) => {
                status.wallet_task_errors += 1;
                eprintln!("  wallet setup failed: {error}");
            }
        }
    }
    status.wallets_connected = ready.len();
    eprintln!(
        "  running {}/{} wallets × {steps} steps (one connection each)...",
        status.wallets_connected, status.wallets_configured,
    );

    let start_line = Arc::new(Barrier::new(ready.len() + 1));
    let tasks = ready
        .into_iter()
        .map(|(client, workload)| {
            let start_line = Arc::clone(&start_line);
            tokio::spawn(async move {
                start_line.wait().await;
                match workload {
                    WalletWorkload::Ranges(start) => {
                        run_wallet(client, start, chunk, steps, tree_every, timeout).await
                    }
                    WalletWorkload::Trees { heights, index } => {
                        run_tree_wallet(client, heights, index, steps, timeout).await
                    }
                    WalletWorkload::Txs { txs, index } => {
                        run_tx_wallet(client, txs, index, steps, timeout).await
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    let wall_start = Instant::now();
    start_line.wait().await;
    let results = futures::future::join_all(tasks).await;
    let wall_secs = wall_start.elapsed().as_secs_f64();

    let mut samples = Vec::new();
    for result in results {
        match result {
            Ok(wallet_samples) => samples.extend(wallet_samples),
            Err(error) => {
                status.wallet_task_errors += 1;
                eprintln!("  wallet measurement failed: {error}");
            }
        }
    }
    Ok(summarize(name, &samples, wall_secs, status))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.endpoint.is_empty() {
        eprintln!("error: at least one --endpoint name=url is required");
        eprintln!("example: --endpoint lwd=http://127.0.0.1:9067 --endpoint zainod=http://127.0.0.1:8137");
        std::process::exit(1);
    }
    if args.tx && args.start_height.checked_add(TX_SOURCE_BLOCKS).is_none() {
        eprintln!("error: --start-height is too large for --tx");
        std::process::exit(1);
    }

    let timeout = Duration::from_secs(args.timeout);
    let tree_heights = args
        .tree_heights_file
        .as_deref()
        .map(read_tree_heights)
        .transpose()?;
    let mut all_summaries = Vec::new();
    let mut failed = false;

    for (name, url) in &args.endpoint {
        eprintln!("Connecting to {} ({})...", name, url);

        // build the endpoint template (TLS config applied per connection below)
        let mut endpoint = Channel::from_shared(url.clone())?.timeout(timeout);
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

        let summary = match run_endpoint(name, endpoint, &args, tree_heights.clone(), timeout).await {
            Ok(summary) => summary,
            Err(_) => {
                // already reported; no result is recorded for this endpoint
                failed = true;
                continue;
            }
        };
        if !args.json {
            print_summary(&summary);
            println!(
                "  wall time: {:.2}s   throughput: {:.0} req/s",
                summary.wall_time_secs, summary.throughput_rps
            );
            println!();
        }
        all_summaries.push(summary);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&all_summaries)?);
    }
    if let Some(path) = &args.json_out {
        if all_summaries.is_empty() {
            eprintln!("no results to write to {}", path);
        } else {
            std::fs::write(path, serde_json::to_string_pretty(&all_summaries)?)?;
            eprintln!("wrote {}", path);
        }
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}
