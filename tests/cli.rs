use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::Stream;
use serde_json::Value;
use tokio::{net::TcpListener, process::Command};
use tonic::{transport::Server, Request, Response, Status};

mod rpc {
    tonic::include_proto!("cash.z.wallet.sdk.rpc");
}

use rpc::{
    compact_tx_streamer_server::{CompactTxStreamer, CompactTxStreamerServer},
    BlockId, BlockRange, ChainSpec, CompactBlock, LightdInfo, TreeState,
};

#[derive(Default)]
struct Calls {
    trees: Vec<u64>,
    ranges: Vec<(u64, u64)>,
    probes: usize,
}

#[derive(Clone, Copy)]
enum Behavior {
    Normal,
    FailTreeAndProbe,
}

#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Calls>>,
    behavior: Behavior,
}

#[tonic::async_trait]
impl CompactTxStreamer for Mock {
    async fn get_latest_block(&self, _: Request<ChainSpec>) -> Result<Response<BlockId>, Status> {
        Err(Status::unimplemented("GetLatestBlock"))
    }

    type GetBlockRangeStream =
        std::pin::Pin<Box<dyn Stream<Item = Result<CompactBlock, Status>> + Send>>;

    async fn get_block_range(
        &self,
        request: Request<BlockRange>,
    ) -> Result<Response<Self::GetBlockRangeStream>, Status> {
        let range = request.into_inner();
        let start = range.start.unwrap().height;
        let end = range.end.unwrap().height;
        self.calls.lock().unwrap().ranges.push((start, end));
        let blocks = (start..=end).map(|height| {
            Ok(CompactBlock {
                height,
                ..Default::default()
            })
        });
        Ok(Response::new(Box::pin(futures::stream::iter(blocks))))
    }

    async fn get_tree_state(
        &self,
        request: Request<BlockId>,
    ) -> Result<Response<TreeState>, Status> {
        let id = request.into_inner();
        assert!(id.hash.is_empty());
        self.calls.lock().unwrap().trees.push(id.height);
        let height = match self.behavior {
            Behavior::FailTreeAndProbe => {
                let mut status = Status::unavailable("tree unavailable");
                status
                    .metadata_mut()
                    .insert("x-height", id.height.to_string().parse().unwrap());
                return Err(status);
            }
            Behavior::Normal => id.height,
        };
        Ok(Response::new(TreeState {
            height,
            ..Default::default()
        }))
    }

    async fn get_lightd_info(&self, _: Request<ChainSpec>) -> Result<Response<LightdInfo>, Status> {
        self.calls.lock().unwrap().probes += 1;
        if matches!(self.behavior, Behavior::FailTreeAndProbe) {
            return Err(Status::unimplemented("probe unavailable"));
        }
        Ok(Response::new(LightdInfo::default()))
    }
}

async fn serve(name: &str, behavior: Behavior) -> (String, Arc<Mutex<Calls>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let service = Mock {
        calls: Arc::clone(&calls),
        behavior,
    };
    tokio::spawn(async move {
        let incoming = futures::stream::unfold(listener, |listener| async move {
            let accepted = listener.accept().await.map(|(stream, _)| stream);
            Some((accepted, listener))
        });
        Server::builder()
            .add_service(CompactTxStreamerServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    (format!("{name}=http://{address}"), calls)
}

async fn run(endpoints: &[&str], args: &[&str]) -> Output {
    tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(env!("CARGO_BIN_EXE_cbb"))
            .kill_on_drop(true)
            .arg("--json")
            .args(
                endpoints
                    .iter()
                    .flat_map(|endpoint| ["--endpoint", *endpoint]),
            )
            .args(args)
            .output(),
    )
    .await
    .expect("benchmark timed out")
    .unwrap()
}

fn summary_json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {:?}", String::from_utf8_lossy(&output.stdout)))
}

#[tokio::test]
async fn json_reports_unreachable_endpoints_failed_probes_and_all_error_methods() {
    let (mixed_endpoint, calls) = serve("mixed", Behavior::FailTreeAndProbe).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let missing_endpoint = format!("missing=http://{}", listener.local_addr().unwrap());
    drop(listener);
    let summaries = summary_json(
        run(
            &[&missing_endpoint, &mixed_endpoint],
            &[
                "--concurrency",
                "1",
                "--steps",
                "6",
                "--chunk",
                "1",
                "--warmup",
                "0",
                "--timeout",
                "1",
            ],
        )
        .await,
    );

    let missing = &summaries[0];
    assert_eq!(missing["name"], "missing");
    assert_eq!(missing["wallets_configured"], 1);
    assert_eq!(missing["wallets_connected"], 0);
    assert_eq!(missing["wallet_connection_errors"], 1);
    assert!(missing["probe_error"].is_string());
    assert_eq!(missing["total_requests"], 0);

    let mixed = &summaries[1];
    assert_eq!(mixed["name"], "mixed");
    assert_eq!(mixed["wallets_configured"], 1);
    assert_eq!(mixed["wallets_connected"], 1);
    assert_eq!(mixed["wallet_connection_errors"], 0);
    assert!(mixed["probe_error"]
        .as_str()
        .unwrap()
        .contains("probe unavailable"));
    assert_eq!(mixed["total_requests"], 8);
    assert_eq!(mixed["errors"], 2);
    let tree = mixed["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|method| method["method"] == "GetTreeState")
        .unwrap();
    assert_eq!(tree["count"], 0);
    assert_eq!(tree["errors"], 2);
    assert_eq!(tree["p95"], 0.0);
    assert_eq!(
        mixed["error_samples"],
        serde_json::json!([["Unavailable: tree unavailable", 2]])
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.probes, 1);
    assert_eq!(
        calls.ranges,
        (2_000_000..=2_000_005)
            .map(|height| (height, height))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn tree_every_zero_keeps_range_warmup_and_measurement() {
    let (endpoint, calls) = serve("range", Behavior::Normal).await;
    let summaries = summary_json(
        run(
            &[&endpoint],
            &[
                "--concurrency",
                "1",
                "--steps",
                "2",
                "--chunk",
                "1",
                "--warmup",
                "1",
                "--tree-every",
                "0",
            ],
        )
        .await,
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.probes, 1);
    assert_eq!(
        calls.ranges,
        [
            (2_000_000, 2_000_000),
            (2_000_001, 2_000_001),
            (2_000_002, 2_000_002)
        ]
    );
    assert!(calls.trees.is_empty());
    assert_eq!(summaries[0]["total_requests"], 2);
    assert_eq!(summaries[0]["probe_error"], Value::Null);
    assert_eq!(summaries[0]["methods"].as_array().unwrap().len(), 1);
    assert_eq!(summaries[0]["methods"][0]["method"], "GetBlockRange");
}

#[tokio::test]
async fn a_stalled_listener_cannot_hold_the_benchmark_open() {
    let stalled = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("stalled=http://{}", stalled.local_addr().unwrap());
    let summaries = summary_json(
        run(
            &[&endpoint],
            &[
                "--concurrency",
                "1",
                "--steps",
                "1",
                "--tree-every",
                "0",
                "--warmup",
                "0",
                "--timeout",
                "1",
            ],
        )
        .await,
    );
    let summary = &summaries[0];
    assert_eq!(summary["wallets_configured"], 1);
    assert!(summary["probe_error"].is_string());
    assert_eq!(summary["throughput_rps"], 0.0);
    assert_eq!(
        summary["wallet_connection_errors"].as_u64().unwrap() + summary["errors"].as_u64().unwrap(),
        1
    );
}
