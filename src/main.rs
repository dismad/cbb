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
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
    chunk: u64,

    /// Measured range or tree-only requests per wallet
    #[arg(long, default_value_t = 20)]
    steps: u64,

    /// Direction of each GetBlockRange request; both cover the same height intervals
    #[arg(long, value_enum, default_value_t = RangeOrder::Asc)]
    range_order: RangeOrder,

    /// Starting height for the first wallet (others are offset)
    #[arg(long, default_value_t = 2_000_000)]
    start_height: u64,

    /// Also call GetTreeState every N range requests (0 = never)
    #[arg(long, default_value_t = 5)]
    tree_every: u64,

    /// Run only GetTreeState with one unsigned decimal height per line
    #[arg(long, value_name = "PATH")]
    tree_heights_file: Option<PathBuf>,

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

    /// Exit nonzero when work is missing or a probe, RPC, connection, or task fails
    #[arg(long)]
    strict: bool,

    /// Before measuring, require GetLightdInfo to report this chain name
    #[arg(long, value_name = "NAME")]
    expect_chain: Option<String>,

    /// Before measuring, require GetLatestBlock to report HEIGHT:HASH_HEX (hash bytes as served)
    #[arg(long, value_name = "HEIGHT:HASH_HEX", value_parser = parse_tip)]
    expect_tip: Option<(u64, Vec<u8>)>,
}

impl Args {
    fn checks_identity(&self) -> bool {
        self.expect_chain.is_some() || self.expect_tip.is_some()
    }

    fn ranges_fit(&self) -> bool {
        let last_start = (self.concurrency.saturating_sub(1) as u64)
            .checked_mul(10)
            .and_then(|offset| self.start_height.checked_add(offset));
        let blocks = self.warmup.checked_add(self.steps).and_then(|steps| steps.checked_mul(self.chunk));
        last_start.zip(blocks)
            .and_then(|(start, blocks)| start.checked_add(blocks.saturating_sub(1)))
            .is_some()
    }

    fn expected_requests(&self) -> [(&'static str, Option<u64>); 2] {
        let (ranges, trees) = if self.tree_heights_file.is_some() {
            (0, self.steps)
        } else if self.tree_every > 0 && self.steps > 0 {
            (self.steps, 1 + (self.steps - 1) / self.tree_every)
        } else {
            (self.steps, 0)
        };
        let wallets = self.concurrency as u64;
        [
            ("GetBlockRange", ranges.checked_mul(wallets)),
            ("GetTreeState", trees.checked_mul(wallets)),
        ]
    }
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum RangeOrder {
    Asc,
    Desc,
}

fn parse_endpoint(s: &str) -> Result<(String, String), String> {
    let (name, url) = s.split_once('=').ok_or("expected name=url")?;
    if name.is_empty() || url.is_empty() {
        return Err("name and url must be non-empty".into());
    }
    Ok((name.to_string(), url.to_string()))
}

fn parse_tip(s: &str) -> Result<(u64, Vec<u8>), String> {
    let (height, hash) = s.split_once(':').ok_or("expected HEIGHT:HASH_HEX")?;
    let height = height.parse().map_err(|_| "height must be an unsigned decimal")?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("hash must be 64 hex digits".into());
    }
    let bytes = (0..hash.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hash[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    Ok((height, bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
    stream: Option<StreamStats>,
}

/// Timings of one complete GetBlockRange stream, from the request to its last block.
#[derive(Debug, Clone, Copy)]
struct StreamStats {
    blocks: u64,
    first_block_ms: f64,
    /// Longest wait between consecutive blocks; `None` for a one-block stream.
    max_gap_ms: Option<f64>,
}

/// One GetBlockRange over `height..height + chunk`, read to its last block. Requests the
/// interval ascending or descending and fails unless the stream arrives in exactly that
/// order, on an error status, or on the timeout.
async fn fetch_range(
    client: &mut CompactTxStreamerClient<Channel>,
    height: u64,
    chunk: u64,
    order: RangeOrder,
    timeout: Duration,
) -> Result<StreamStats, String> {
    let end = height + (chunk - 1);
    let (first, last) = match order {
        RangeOrder::Asc => (height, end),
        RangeOrder::Desc => (end, height),
    };
    let req = BlockRange {
        start: Some(BlockId { height: first, hash: vec![] }),
        end: Some(BlockId { height: last, hash: vec![] }),
    };
    let drain = async {
        let began = Instant::now();
        let mut stream = client.get_block_range(req).await.map_err(rpc_error)?.into_inner();
        let (mut received, mut previous) = (0, began);
        let mut stats = StreamStats { blocks: 0, first_block_ms: 0.0, max_gap_ms: None };
        while let Some(block) = stream.message().await.map_err(rpc_error)? {
            let now = Instant::now();
            if received == chunk {
                return Err("stream returned more blocks than the range".to_string());
            }
            let expected = match order {
                RangeOrder::Asc => height + received,
                RangeOrder::Desc => end - received,
            };
            if block.height != expected {
                return Err("block out of order".to_string());
            }
            if received == 0 {
                stats.first_block_ms = (now - began).as_secs_f64() * 1000.0;
            } else {
                let gap = (now - previous).as_secs_f64() * 1000.0;
                stats.max_gap_ms = Some(stats.max_gap_ms.map_or(gap, |max| max.max(gap)));
            }
            previous = now;
            received += 1;
        }
        if received != chunk {
            return Err("stream ended before the range did".to_string());
        }
        stats.blocks = received;
        Ok(stats)
    };
    tokio::time::timeout(timeout, drain).await.unwrap_or_else(|_| Err("timeout".to_string()))
}

async fn run_wallet(
    mut client: CompactTxStreamerClient<Channel>,
    start_height: u64,
    chunk: u64,
    steps: u64,
    tree_every: u64,
    order: RangeOrder,
    timeout: Duration,
) -> Vec<Sample> {
    let mut samples = Vec::new();
    for i in 0..steps {
        let height = start_height + i * chunk;
        let start = Instant::now();
        let result = fetch_range(&mut client, height, chunk, order, timeout).await;
        let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
        let (error, stream) = match result {
            Ok(stream) => (None, Some(stream)),
            Err(error) => (Some(error), None),
        };
        samples.push(Sample { method: "GetBlockRange", latency_ms, error, stream });

        if tree_every > 0 && i % tree_every == 0 {
            let start = Instant::now();
            let result = check_tree(&mut client, height, timeout).await;
            samples.push(Sample {
                method: "GetTreeState",
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                error: result.err(),
                stream: None,
            });
        }

    }

    samples
}

async fn check_tree(
    client: &mut CompactTxStreamerClient<Channel>,
    height: u64,
    timeout: Duration,
) -> Result<(), String> {
    let id = BlockId { height, hash: vec![] };
    let actual = tokio::time::timeout(timeout, client.get_tree_state(id))
        .await
        .map_err(|_| "timeout".to_string())?
        .map_err(rpc_error)?
        .into_inner()
        .height;
    if actual != height {
        return Err(format!("GetTreeState returned height {actual}, expected {height}"));
    }
    Ok(())
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
        let result = check_tree(&mut client, height, timeout).await;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        samples.push(Sample { method: "GetTreeState", latency_ms: elapsed, error: result.err(), stream: None });
        index = (index + 1) % heights.len();
    }
    samples
}

fn summarize(name: &str, samples: &[Sample], wall_secs: f64, status: RunStatus) -> Summary {
    let mut hist = Histogram::<u64>::new(3).unwrap();
    let mut errors = 0;
    let mut error_msgs = HashMap::new();
    let mut by_method: HashMap<&str, (Vec<f64>, u64, Vec<StreamStats>)> = HashMap::new();

    for sample in samples {
        let (latencies, method_errors, streams) = by_method.entry(sample.method).or_default();
        if let Some(message) = &sample.error {
            errors += 1;
            *method_errors += 1;
            *error_msgs.entry(message.clone()).or_default() += 1;
        } else {
            let _ = hist.record((sample.latency_ms * 1000.0) as u64);
            latencies.push(sample.latency_ms);
            streams.extend(sample.stream);
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
        throughput_rps: per_second(total - errors, wall_secs),
        methods: by_method
            .into_iter()
            .map(|(method, (latencies, errors, streams))| {
                let mut h = Histogram::<u64>::new(3).unwrap();
                for x in &latencies {
                    let _ = h.record((*x * 1000.0) as u64);
                }
                let blocks = (!streams.is_empty()).then(|| streams.iter().map(|s| s.blocks).sum());
                MethodSummary {
                    method: method.to_string(),
                    count: latencies.len() as u64,
                    errors,
                    requests_per_sec: per_second(latencies.len() as u64, wall_secs),
                    p50: h.value_at_quantile(0.50) as f64 / 1000.0,
                    p95: h.value_at_quantile(0.95) as f64 / 1000.0,
                    p99: h.value_at_quantile(0.99) as f64 / 1000.0,
                    max: h.max() as f64 / 1000.0,
                    first_block: stream_quantiles(streams.iter().map(|s| s.first_block_ms)),
                    max_block_gap: stream_quantiles(streams.iter().filter_map(|s| s.max_gap_ms)),
                    blocks,
                    blocks_per_sec: blocks.map(|count| per_second(count, wall_secs)),
                }
            })
            .collect(),
        error_samples: error_msgs.into_iter().collect(),
        status,
    }
}

fn per_second(count: u64, seconds: f64) -> f64 {
    if seconds > 0.0 { count as f64 / seconds } else { 0.0 }
}

/// Latency percentiles in milliseconds.
#[derive(Debug, serde::Serialize)]
struct Quantiles {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

fn stream_quantiles(values: impl Iterator<Item = f64>) -> Option<Quantiles> {
    let mut values = values.peekable();
    values.peek()?;
    let mut h = Histogram::<u64>::new(3).unwrap();
    for value in values {
        let _ = h.record((value * 1000.0) as u64);
    }
    Some(Quantiles {
        p50: h.value_at_quantile(0.50) as f64 / 1000.0,
        p95: h.value_at_quantile(0.95) as f64 / 1000.0,
        p99: h.value_at_quantile(0.99) as f64 / 1000.0,
        max: h.max() as f64 / 1000.0,
    })
}

/// For `GetBlockRange`, `p50` to `max` are full-stream completion times. The stream fields
/// below cover successful streams only and are absent for other methods.
#[derive(Debug, serde::Serialize)]
struct MethodSummary {
    method: String,
    count: u64,
    errors: u64,
    requests_per_sec: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    /// Time from the request to the first block.
    #[serde(skip_serializing_if = "Option::is_none")]
    first_block: Option<Quantiles>,
    /// Longest wait between blocks within a stream, over streams of two or more blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_block_gap: Option<Quantiles>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks: Option<u64>,
    /// `blocks` divided by the run's wall time.
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks_per_sec: Option<f64>,
}

#[derive(Debug, serde::Serialize)]
struct RunStatus {
    wallets_configured: usize,
    wallets_connected: usize,
    wallet_connection_errors: usize,
    wallet_task_errors: usize,
    warmup_errors: usize,
    probe_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    range_order: Option<RangeOrder>,
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
    if s.status.warmup_errors > 0 {
        println!("  warm-up errors: {}", s.status.warmup_errors);
    }
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
    for m in &s.methods {
        let (Some(blocks), Some(rate)) = (m.blocks, m.blocks_per_sec) else { continue };
        println!("  {} streams: {blocks} blocks ({rate:.0}/s)", m.method);
        for (label, q) in [("first block", &m.first_block), ("max block gap", &m.max_block_gap)] {
            if let Some(q) = q {
                println!("    {label:<14} p50 {:.1}ms  p95 {:.1}ms  p99 {:.1}ms  max {:.1}ms",
                         q.p50, q.p95, q.p99, q.max);
            }
        }
        println!();
    }
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

async fn probe(endpoint: &Endpoint, args: &Args, timeout: Duration) -> Result<(), String> {
    let mut client = connect(endpoint, timeout)
        .await
        .map_err(|error| format!("probe connection failed: {error}"))?;
    let response = tokio::time::timeout(timeout, client.get_lightd_info(ChainSpec {}))
        .await
        .map_err(|_| "GetLightdInfo timed out".to_owned())?
        .map_err(|error| format!("GetLightdInfo failed: {}", rpc_error(error)))?;
    let info = response.into_inner();
    eprintln!("  vendor={} chain={} height={}", info.vendor, info.chain_name, info.block_height);
    if let Some(chain) = args.expect_chain.as_deref().filter(|chain| *chain != info.chain_name) {
        return Err(format!("expected chain {chain}, server reports {}", info.chain_name));
    }
    if let Some((height, hash)) = &args.expect_tip {
        let tip = tokio::time::timeout(timeout, client.get_latest_block(ChainSpec {}))
            .await
            .map_err(|_| "GetLatestBlock timed out".to_owned())?
            .map_err(|error| format!("GetLatestBlock failed: {}", rpc_error(error)))?
            .into_inner();
        if tip.height != *height || tip.hash != *hash {
            return Err(format!(
                "expected tip {height}:{}, server reports {}:{}",
                hex(hash),
                tip.height,
                hex(&tip.hash)
            ));
        }
    }
    Ok(())
}

enum WalletWorkload {
    Ranges(u64),
    Trees { heights: Arc<[u64]>, index: usize },
}

async fn run_endpoint(
    name: &str,
    endpoint: Endpoint,
    args: &Args,
    tree_heights: Option<Arc<[u64]>>,
    timeout: Duration,
) -> Summary {
    let order = args.range_order;
    let probe_error = if tree_heights.is_some() && !args.checks_identity() {
        None
    } else {
        probe(&endpoint, args, timeout).await.err()
    };
    let mut status = RunStatus {
        wallets_configured: args.concurrency,
        wallets_connected: 0,
        wallet_connection_errors: 0,
        wallet_task_errors: 0,
        warmup_errors: 0,
        probe_error,
        range_order: tree_heights.is_none().then_some(order),
    };
    if let Some(error) = &status.probe_error {
        eprintln!("  {error}");
        if args.checks_identity() {
            return summarize(name, &[], 0.0, status);
        }
    }

    let chunk = args.chunk;
    let steps = args.steps;
    let tree_every = args.tree_every;
    let warmup = args.warmup;
    let start_height = args.start_height;
    let setup = (0..args.concurrency).map(|wallet| {
        let endpoint = endpoint.clone();
        let tree_heights = tree_heights.clone();
        tokio::spawn(async move {
            let mut client = connect(&endpoint, timeout).await?;
            let mut warmup_errors = 0;
            let workload = if let Some(heights) = tree_heights {
                let index = (wallet as u128 * u128::from(steps) % heights.len() as u128) as usize;
                for _ in 0..warmup {
                    warmup_errors +=
                        usize::from(check_tree(&mut client, heights[index], timeout).await.is_err());
                }
                WalletWorkload::Trees { heights, index }
            } else {
                let start = start_height + wallet as u64 * 10;
                for step in 0..warmup {
                    let result = fetch_range(&mut client, start + step * chunk, chunk, order, timeout).await;
                    warmup_errors += usize::from(result.is_err());
                }
                WalletWorkload::Ranges(if steps > 0 { start + warmup * chunk } else { start })
            };
            Ok::<_, String>((client, workload, warmup_errors))
        })
    });

    let mut ready = Vec::new();
    for result in futures::future::join_all(setup).await {
        match result {
            Ok(Ok((client, workload, warmup_errors))) => {
                status.warmup_errors += warmup_errors;
                ready.push((client, workload));
            }
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
                        run_wallet(client, start, chunk, steps, tree_every, order, timeout).await
                    }
                    WalletWorkload::Trees { heights, index } => {
                        run_tree_wallet(client, heights, index, steps, timeout).await
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
    summarize(name, &samples, wall_secs, status)
}

impl Summary {
    fn has_failures(&self, args: &Args) -> bool {
        self.total_requests == 0
            || args.expected_requests().into_iter().any(|(method, expected)| {
                let completed = self.methods.iter().find(|report| report.method == method)
                    .map_or(0, |report| report.count);
                Some(completed) != expected
            })
            || self.errors > 0
            || self.status.wallets_connected != self.status.wallets_configured
            || self.status.wallet_task_errors > 0
            || self.status.warmup_errors > 0
            || self.status.probe_error.is_some()
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
    let tree_heights = args
        .tree_heights_file
        .as_deref()
        .map(read_tree_heights)
        .transpose()?;
    if tree_heights.is_none() && args.concurrency > 0 && !args.ranges_fit() {
        return Err("block range workload exceeds the maximum height".into());
    }
    let mut all_summaries = Vec::new();

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

        let summary = run_endpoint(name, endpoint, &args, tree_heights.clone(), timeout).await;
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
        std::fs::write(path, serde_json::to_string_pretty(&all_summaries)?)?;
        eprintln!("wrote {}", path);
    }

    let mut failed = false;
    for summary in &all_summaries {
        let identity_failed = args.checks_identity() && summary.status.probe_error.is_some();
        if identity_failed || (args.strict && summary.has_failures(&args)) {
            eprintln!("{}: benchmark did not complete successfully", summary.name);
            failed = true;
        }
    }
    if failed {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_requires_the_complete_mixed_workload() {
        let args = Args::parse_from(["cbb", "--concurrency", "2", "--steps", "6", "--tree-every", "5"]);
        let status = || RunStatus {
            wallets_configured: 2,
            wallets_connected: 2,
            wallet_connection_errors: 0,
            wallet_task_errors: 0,
            warmup_errors: 0,
            probe_error: None,
            range_order: None,
        };
        let mut samples = vec![Sample {
            method: "GetBlockRange",
            latency_ms: 1.0,
            error: None,
            stream: None,
        }; 16];
        for sample in &mut samples[12..] {
            sample.method = "GetTreeState";
        }

        assert!(!summarize("test", &samples, 1.0, status()).has_failures(&args));
        samples[15].method = "GetBlockRange";
        assert!(summarize("test", &samples, 1.0, status()).has_failures(&args));
    }

    #[test]
    fn stream_percentiles_include_long_samples_and_agree_with_maximum() {
        let quantiles = stream_quantiles([65_000.0].into_iter()).unwrap();
        assert!(quantiles.p99 >= 65_000.0 && quantiles.p99 <= quantiles.max);
    }
}
