//! Fixed signed input production and RPC/chain observation for a local dev node.

mod corpus;
mod observe;

use super::{BenchResult, bench_error};
use std::path::Path;

pub(super) fn command(args: Vec<String>) -> Option<BenchResult<()>> {
    match args.first().map(String::as_str) {
        Some("external-corpus") => Some((|| {
            let [_, funding, output, count, inputs] = args.as_slice() else {
                return Err(bench_error(
                    "external-corpus FUNDING_JSON OUTPUT_JSONL COUNT INPUTS",
                ));
            };
            corpus::produce(
                Path::new(funding),
                Path::new(output),
                count.parse()?,
                inputs.parse()?,
            )
        })()),
        Some("external-run") => Some((|| {
            let [_, corpus, url, concurrency, poll_ms, timeout_s, rate @ ..] = args.as_slice()
            else {
                return Err(bench_error(
                    "external-run CORPUS_JSONL RPC_URL CONCURRENCY POLL_MS TIMEOUT_S [ARRIVAL_RATE]",
                ));
            };
            let arrival_rate = match rate {
                [] => None,
                [rate] => Some(rate.parse()?),
                _ => return Err(bench_error("external-run accepts at most one arrival rate")),
            };
            observe::run(
                Path::new(corpus),
                url,
                concurrency.parse()?,
                poll_ms.parse()?,
                timeout_s.parse()?,
                arrival_rate,
            )
        })()),
        _ => None,
    }
}
