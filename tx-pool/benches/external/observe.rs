//! Submit a frozen corpus while independently observing the canonical chain.
//!
//! An observation interval brackets visibility, not the node's commit timestamp.
//! Its lower end is the start of the last tip RPC excluding the block; its upper
//! end is completion of the first block RPC containing the transaction.

use super::super::require;
use super::{BenchResult, bench_error, corpus::Corpus};
use ckb_types::bytes::Bytes;
use futures_util::{StreamExt, stream};
use http_body_util::{BodyExt, Full};
use hyper::{Request, Uri};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, io::Write, path::Path, sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};

struct Rpc {
    client: Client<HttpConnector, Full<Bytes>>,
    uri: Uri,
}

impl Rpc {
    fn new(url: &str) -> BenchResult<Self> {
        let uri: Uri = url.parse()?;
        require(
            uri.scheme_str() == Some("http")
                && matches!(uri.host(), Some("127.0.0.1" | "localhost" | "[::1]")),
            "external benchmark requires a local HTTP dev node",
        )?;
        Ok(Self {
            client: Client::builder(hyper_util::rt::TokioExecutor::new())
                .retry_canceled_requests(false)
                .build_http(),
            uri,
        })
    }

    async fn request(&self, body: Bytes) -> BenchResult<Value> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let request = Request::post(self.uri.clone())
                .header("content-type", "application/json")
                .body(Full::new(body))?;
            let response = self.client.request(request).await?;
            require(
                response.status().is_success(),
                format!("RPC HTTP status {}", response.status()),
            )?;
            let response: Value =
                serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
            require(
                response["jsonrpc"] == "2.0"
                    && response["id"] == 1
                    && (response.get("result").is_some() != response.get("error").is_some()),
                "invalid RPC response envelope",
            )?;
            Ok(response)
        })
        .await
        .map_err(|_| bench_error("RPC exceeded 30 seconds"))?
    }

    async fn call(&self, method: &str, params: Value) -> BenchResult<Value> {
        let body = request_body(method, params)?;
        let response = self.request(body).await?;
        match response.get("result") {
            Some(value) => Ok(value.clone()),
            None => Err(bench_error(format!("{method}: {}", response["error"]))),
        }
    }
}

fn request_body(method: &str, params: Value) -> BenchResult<Bytes> {
    Ok(
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))?
            .into(),
    )
}

fn nanos(start: Instant) -> u64 {
    // The bounded run duration is far below the u64 nanosecond range.
    start.elapsed().as_nanos() as u64
}

#[derive(Default)]
struct Submission {
    scheduled: Option<u64>,
    started: Option<u64>,
    ended: Option<u64>,
    response: Option<Value>,
}

struct PreparedTransaction {
    hash: String,
    request: Bytes,
}

async fn submit(
    rpc: &Rpc,
    requests: &[PreparedTransaction],
    submissions: &mut [Submission],
    concurrency: usize,
    start: Instant,
) -> BenchResult<()> {
    let mut requests = stream::iter(requests.iter().zip(submissions.iter_mut()))
        .map(|(transaction, record)| async move {
            if let Some(scheduled) = record.scheduled {
                // Capacity can delay a request, but cannot move its arrival.
                // The retained schedule exposes that client-side queueing.
                tokio::time::sleep_until(start + Duration::from_nanos(scheduled)).await;
            }
            record.started = Some(nanos(start));
            let response = rpc.request(transaction.request.clone()).await;
            record.ended = Some(nanos(start));
            record.response = Some(match response {
                Ok(response) => response,
                Err(error) => json!({"transport_error":error.to_string()}),
            });
            record
                .response
                .as_ref()
                .is_some_and(|response| response["result"] == transaction.hash)
        })
        .buffer_unordered(concurrency);
    let mut accepted = true;
    while let Some(success) = requests.next().await {
        accepted &= success;
    }
    require(
        accepted,
        "one or more submissions failed; retain every RPC result",
    )
}

fn block_number(header: &Value) -> BenchResult<u64> {
    let number = header["number"]
        .as_str()
        .and_then(|value| value.strip_prefix("0x"))
        .ok_or_else(|| bench_error("header has no hexadecimal block number"))?;
    Ok(u64::from_str_radix(number, 16)?)
}

async fn observe(
    rpc: &Rpc,
    metadata: &Value,
    expected: &HashSet<String>,
    start: Instant,
    poll_ms: u64,
    blocks: &mut Vec<Value>,
) -> BenchResult<()> {
    let mut seen = HashSet::new();
    let mut height = metadata["tip_number"]
        .as_u64()
        .ok_or_else(|| bench_error("missing seed height"))?;
    let mut previous = metadata["tip_hash"].clone();
    let mut last_negative = 0;
    loop {
        let poll_start = nanos(start);
        let tip = rpc.call("get_tip_header", json!([])).await?;
        let poll_end = nanos(start);
        let tip_height = block_number(&tip)?;
        require(tip_height >= height, "observed chain height rolled back")?;
        if tip_height == height {
            require(
                tip["hash"] == previous,
                "observed a replacement canonical tip",
            )?;
        }
        for number in height + 1..=tip_height {
            let fetch_start = nanos(start);
            let block = rpc
                .call("get_block_by_number", json!([format!("{number:#x}")]))
                .await?;
            let fetch_end = nanos(start);
            require(
                block_number(&block["header"])? == number
                    && block["header"]["parent_hash"] == previous,
                "observed block does not extend the captured canonical prefix",
            )?;
            let transactions = block["transactions"]
                .as_array()
                .ok_or_else(|| bench_error("block has no transactions"))?;
            require(!transactions.is_empty(), "block has no cellbase")?;
            let mut hashes = Vec::with_capacity(transactions.len() - 1);
            for transaction in transactions.iter().skip(1) {
                let hash = transaction["hash"]
                    .as_str()
                    .ok_or_else(|| bench_error("block transaction has no hash"))?;
                require(
                    expected.contains(hash),
                    "canonical block contains an unexpected non-cellbase transaction",
                )?;
                require(
                    seen.insert(hash.to_owned()),
                    "transaction was confirmed more than once",
                )?;
                hashes.push(hash);
            }
            previous = block["header"]["hash"].clone();
            blocks.push(json!({"number":number,"hash":previous,"parent_hash":block["header"]["parent_hash"],
                "last_negative_tip_start_ns":last_negative,"tip_rpc_start_ns":poll_start,"tip_rpc_end_ns":poll_end,
                "block_rpc_start_ns":fetch_start,"first_positive_block_end_ns":fetch_end,"transactions":hashes}));
        }
        require(
            previous == tip["hash"],
            "tip and fetched canonical prefix differ",
        )?;
        height = tip_height;
        last_negative = poll_start;
        if seen.len() == expected.len() {
            let canonical = rpc
                .call("get_block_hash", json!([format!("{height:#x}")]))
                .await?;
            require(
                canonical == previous,
                "final observed prefix is no longer canonical",
            )?;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
    }
}

/// Stop both tasks on the first failure or their shared deadline. Each task
/// owns its records outside this cancellable future and returns them afterward.
async fn until_stopped(
    operation: impl Future<Output = BenchResult<()>>,
    failure: &watch::Sender<Option<String>>,
    deadline: Instant,
) {
    let mut stopped = failure.subscribe();
    if stopped.borrow().is_some() {
        return;
    }
    let outcome = tokio::select! {
        result = operation => result,
        _ = tokio::time::sleep_until(deadline) => Err(bench_error("external run exceeded deadline")),
        _ = stopped.changed() => return,
    };
    if let Err(error) = outcome {
        failure.send_if_modified(|first| {
            if first.is_some() {
                return false;
            }
            *first = Some(error.to_string());
            true
        });
    }
}

pub(super) fn run(
    path: &Path,
    url: &str,
    concurrency: usize,
    poll_ms: u64,
    timeout_s: u64,
    arrival_rate: Option<u64>,
) -> BenchResult<()> {
    require(
        (1..=1024).contains(&concurrency)
            && (1..=1000).contains(&poll_ms)
            && (1..=3600).contains(&timeout_s)
            && arrival_rate.is_none_or(|rate| (1..=65_536).contains(&rate)),
        "external run bounds are invalid",
    )?;
    let Corpus {
        metadata,
        transactions,
    } = Corpus::read(path)?;
    let requests = transactions
        .into_iter()
        .map(|transaction| {
            let transaction = ckb_jsonrpc_types::TransactionView::from(transaction);
            Ok(PreparedTransaction {
                hash: format!("{:#x}", transaction.hash),
                request: request_body(
                    "send_transaction",
                    json!([transaction.inner, "passthrough"]),
                )?,
            })
        })
        .collect::<BenchResult<Vec<_>>>()?;
    let expected = requests
        .iter()
        .map(|transaction| transaction.hash.clone())
        .collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let rpc = Arc::new(Rpc::new(url)?);
        let genesis = rpc.call("get_block_hash", json!(["0x0"])).await?;
        let tip = rpc.call("get_tip_header", json!([])).await?;
        let pool = rpc.call("tx_pool_info", json!([])).await?;
        require(genesis == metadata["genesis"] && tip["hash"] == metadata["tip_hash"]
            && Some(block_number(&tip)?) == metadata["tip_number"].as_u64(), "node does not match the frozen starting chain")?;
        require(["pending","proposed","orphan"].iter().all(|field| pool[field] == "0x0"), "starting transaction pool is not empty")?;
        // The owning runner starts the frozen miner only after this readiness
        // event, then releases submission. Signing and request serialization are
        // outside the window; opening submission connections remains inside it.
        println!("EXTERNAL_READY");
        std::io::stdout().flush()?;
        let mut gate = String::new();
        require(std::io::stdin().read_line(&mut gate)? > 0 && gate == "start\n", "submission gate was not released")?;
        let mut submissions: Vec<_> = (0..requests.len())
            .map(|index| Submission {
                // Corpus bounds keep this product within u64. Retain arrivals
                // even for requests never started before failure or timeout.
                scheduled: arrival_rate.map(|rate| index as u64 * 1_000_000_000 / rate),
                ..Submission::default()
            })
            .collect();
        let (failure, _) = watch::channel(None);
        let start = Instant::now();
        let deadline = start + Duration::from_secs(timeout_s);
        // Separate tasks let arrivals progress while the observer parses,
        // validates or drops a large block response. Both use one clock and
        // failure channel; joining retains partial records after cancellation.
        let observing = {
            let rpc = Arc::clone(&rpc);
            let metadata = metadata.clone();
            let failure = failure.clone();
            tokio::spawn(async move {
                let mut blocks = Vec::new();
                until_stopped(
                    observe(&rpc, &metadata, &expected, start, poll_ms, &mut blocks),
                    &failure,
                    deadline,
                ).await;
                blocks
            })
        };
        let (_, blocks) = tokio::join!(
            until_stopped(
                submit(&rpc, &requests, &mut submissions, concurrency, start),
                &failure,
                deadline,
            ),
            observing,
        );
        let blocks = blocks?;
        let outcome = failure.borrow().as_ref()
            .map_or(Ok(()), |error| Err(bench_error(error)));
        let ended = nanos(start);
        // Keep partial records after refusal, transport error, fork or timeout.
        let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
        for (index, (transaction, record)) in requests.iter().zip(&submissions).enumerate() {
            writeln!(writer, "EXTERNAL_SEND {}", json!({"index":index,"hash":transaction.hash,"attempt":0,
                "scheduled_ns":record.scheduled,"submitted_ns":record.started,
                "rpc_completed_ns":record.ended,"response":record.response}))?;
        }
        for block in &blocks { writeln!(writer, "EXTERNAL_BLOCK {block}")?; }
        writeln!(writer, "EXTERNAL_RESULT {}", json!({"schema":2,"outcome":if outcome.is_ok(){"success"}else{"failure"},
            "error":outcome.as_ref().err().map(ToString::to_string),"corpus":metadata,"concurrency":concurrency,
            "poll_ms":poll_ms,"timeout_s":timeout_s,"arrival_rate":arrival_rate,"elapsed_ns":ended,"automatic_retries":0,
            "clock":"process_monotonic","confirmation":"first_observed_canonical_block_with_last_negative_bound",
            "build":{"profiling":cfg!(feature="profiling"),"allocation_observation":cfg!(feature="allocation-observation"),
                "debug_assertions":cfg!(debug_assertions)}}))?;
        writer.flush()?;
        outcome
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[allow(dead_code, reason = "The harness-free benchmark omits test functions.")]
    async fn read_request(stream: &mut tokio::net::TcpStream, body: &[u8]) -> BenchResult<()> {
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            require(headers.len() < 4096, "test request headers exceeded bound")?;
            headers.push(stream.read_u8().await?);
        }
        let mut received = vec![0; body.len()];
        stream.read_exact(&mut received).await?;
        require(
            received == body,
            "test server received a different RPC body",
        )
    }

    #[allow(dead_code, reason = "The harness-free benchmark omits test functions.")]
    async fn respond(stream: &mut tokio::net::TcpStream, hash: &str) -> BenchResult<()> {
        let response = json!({"jsonrpc":"2.0","id":1,"result":hash}).to_string();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{response}",
                    response.len()
                )
                .as_bytes(),
            )
            .await?;
        Ok(())
    }

    #[test]
    fn invalid_arrival_rate_is_rejected_before_reading_the_corpus() {
        for rate in [0, 65_537] {
            let error = run(
                Path::new("unused-corpus"),
                "unused-url",
                1,
                5,
                60,
                Some(rate),
            )
            .unwrap_err();
            assert_eq!(error.to_string(), "external run bounds are invalid");
        }
    }

    #[tokio::test]
    async fn first_failure_cancels_the_other_task_without_losing_its_records() -> BenchResult<()> {
        let (failure, _) = watch::channel(None);
        let deadline = Instant::now() + Duration::from_secs(5);
        let (observed, received) = tokio::sync::oneshot::channel();
        let task_failure = failure.clone();
        let task = tokio::spawn(async move {
            let mut records = Vec::new();
            until_stopped(
                async {
                    records.push("observed before failure");
                    let _ = observed.send(());
                    std::future::pending().await
                },
                &task_failure,
                deadline,
            )
            .await;
            records
        });
        received.await?;
        until_stopped(
            async { Err(bench_error("canonical prefix changed")) },
            &failure,
            deadline,
        )
        .await;
        assert_eq!(task.await?, ["observed before failure"]);
        let mut polled_after_failure = false;
        until_stopped(
            async {
                polled_after_failure = true;
                Err(bench_error("later failure"))
            },
            &failure,
            deadline,
        )
        .await;
        assert!(!polled_after_failure);
        assert_eq!(
            failure.borrow().as_deref(),
            Some("canonical prefix changed")
        );
        Ok(())
    }

    #[tokio::test]
    async fn completion_leaves_the_other_task_running_until_the_shared_deadline() {
        let (failure, _) = watch::channel(None);
        until_stopped(
            async { Ok(()) },
            &failure,
            Instant::now() + Duration::from_secs(5),
        )
        .await;
        assert!(failure.borrow().is_none());
        until_stopped(std::future::pending(), &failure, Instant::now()).await;
        assert_eq!(
            failure.borrow().as_deref(),
            Some("external run exceeded deadline")
        );
    }

    #[tokio::test]
    async fn capacity_delay_preserves_arrivals_and_all_request_outcomes() -> BenchResult<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let rpc = Rpc::new(&format!("http://{}", listener.local_addr()?))?;
        let body = request_body("send_transaction", json!([{}]))?;
        let requests = ["a", "b", "c"].map(|value| PreparedTransaction {
            hash: format!("0x{}", value.repeat(64)),
            request: body.clone(),
        });
        let mut records = [0, 10_000_000, 20_000_000].map(|scheduled| Submission {
            scheduled: Some(scheduled),
            ..Submission::default()
        });
        let start = Instant::now();
        let server = async {
            let (mut connection, _) = listener.accept().await?;
            for (index, request) in requests.iter().enumerate() {
                read_request(&mut connection, &body).await?;
                if index == 0 {
                    // The only in-flight slot stays occupied beyond both later
                    // arrivals. Replying is the event that releases that slot.
                    tokio::time::sleep_until(start + Duration::from_millis(30)).await;
                }
                respond(&mut connection, &request.hash).await?;
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::try_join!(server, submit(&rpc, &requests, &mut records, 1, start))?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        assert!(records[0].ended.unwrap() >= 30_000_000);
        for (index, record) in records.iter().enumerate() {
            assert_eq!(record.scheduled, Some(index as u64 * 10_000_000));
            assert!(record.started >= record.scheduled);
            assert_eq!(
                record.response.as_ref().unwrap()["result"],
                requests[index].hash
            );
        }
        for adjacent in records.windows(2) {
            assert!(adjacent[1].started >= adjacent[0].ended);
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_keeps_the_schedule_for_requests_not_yet_started() -> BenchResult<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let rpc = Rpc::new(&format!("http://{}", listener.local_addr()?))?;
        let body = request_body("send_transaction", json!([{}]))?;
        let requests = ["a", "b"].map(|value| PreparedTransaction {
            hash: format!("0x{}", value.repeat(64)),
            request: body.clone(),
        });
        let mut records = [0, 60_000_000_000].map(|scheduled| Submission {
            scheduled: Some(scheduled),
            ..Submission::default()
        });
        let start = Instant::now();
        let (observed, received) = tokio::sync::oneshot::channel();
        let server = async {
            let (mut connection, _) = listener.accept().await?;
            read_request(&mut connection, &body).await?;
            let _ = observed.send(());
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let client = async {
            // Cancellation follows receipt of the first request, independently
            // of machine speed. Dropping submit also drops its queued futures.
            tokio::select! {
                result = submit(&rpc, &requests, &mut records, 1, start) => {
                    result?;
                    Err(bench_error("pending arrival completed before cancellation"))
                }
                result = received => {
                    result?;
                    Ok(())
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::try_join!(server, client)?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        assert!(records[0].started.is_some());
        assert_eq!(records[1].scheduled, Some(60_000_000_000));
        assert!(records[1].started.is_none());
        assert!(records[1].ended.is_none());
        assert!(records[1].response.is_none());
        Ok(())
    }

    #[test]
    fn closed_reused_connection_retains_the_failed_rpc_without_replaying() -> super::BenchResult<()>
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let rpc = Rpc::new(&format!("http://{}", listener.local_addr()?))?;
            let body = request_body("send_transaction", json!([{}]))?;
            let requests = ["a", "b"].map(|value| PreparedTransaction {
                hash: format!("0x{}", value.repeat(64)),
                request: body.clone(),
            });
            let mut records = [Submission::default(), Submission::default()];
            let (finished, completion) = tokio::sync::oneshot::channel();
            let server = async {
                let (mut connection, _) = listener.accept().await?;
                read_request(&mut connection, &body).await?;
                respond(&mut connection, &requests[0].hash).await?;
                // The second RPC must use this same connection. Closing after
                // its body is read establishes failure without a timing race.
                read_request(&mut connection, &body).await?;
                drop(connection);
                tokio::select! {
                    result = listener.accept() => {
                        result?;
                        Err(bench_error("failed RPC was replayed on another connection"))
                    }
                    result = completion => {
                        result?;
                        Ok(())
                    }
                }
            };
            let client = async {
                let result = submit(&rpc, &requests, &mut records, 1, Instant::now()).await;
                let _ = finished.send(());
                require(result.is_err(), "closed connection did not fail submission")
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::try_join!(server, client)?;
                Ok::<_, Box<dyn std::error::Error>>(())
            })
            .await??;
            require(
                records[0]
                    .response
                    .as_ref()
                    .is_some_and(|response| response["result"] == requests[0].hash)
                    && records[1]
                        .response
                        .as_ref()
                        .is_some_and(|response| response.get("transport_error").is_some())
                    && records[1].started <= records[1].ended,
                "transport failure lost the per-request outcome or clock",
            )
        })
    }
}
