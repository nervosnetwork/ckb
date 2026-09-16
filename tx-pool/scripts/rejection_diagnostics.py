"""Check retained committed-refusal evidence before interpreting a failed run."""

from __future__ import annotations

from collections import Counter
import argparse
import json
from pathlib import Path
import re


HEX_32 = re.compile(r"^[0-9a-f]{64}$")
AMOUNT_KEYS = {"items", "bytes", "edges", "serialized", "cycles"}
ACCOUNT_NAMES = {"accepted", "pipeline", "remote", "peer", "history"}
STAGES = {"ingress", "direct_submission", "resolve", "verify", "waiting", "accepted", "replaced", "policy", "peer_revocation"}


def records(output: str, prefix: str) -> list[dict]:
    return [json.loads(line[len(prefix):]) for line in output.splitlines() if line.startswith(prefix)]


def one_record(output: str, prefix: str) -> dict:
    found = records(output, prefix)
    if len(found) != 1 or not isinstance(found[0], dict):
        raise ValueError(f"expected one {prefix.strip()} record, observed {len(found)}")
    return found[0]


def natural(value: object) -> bool:
    return type(value) is int and 0 <= value <= 2**64 - 1


def validate_events(output: str) -> list[dict]:
    capture = one_record(output, "BENCH_REJECTION_CAPTURE ")
    events = records(output, "BENCH_REJECTION ")
    if (type(capture.get("schema")) is not int or capture["schema"] != 1
            or capture.get("logger") != "rejections_and_warnings_v1"
            or not natural(capture.get("records")) or capture["records"] != len(events)
            or capture.get("write_failed") is not False):
        raise ValueError("rejection capture is incomplete or its output failed")
    service = records(output, "BENCH_SERVICE_LOG ")
    if (not natural(capture.get("service_records")) or capture["service_records"] != len(service)
            or any(not isinstance(value, dict) or value.get("level") not in {"WARN", "ERROR"}
                   or not isinstance(value.get("target"), str) or not isinstance(value.get("message"), str)
                   for value in service)):
        raise ValueError("service warning/error capture is incomplete")
    for event in events:
        if (not isinstance(event, dict) or type(event.get("schema")) is not int or event["schema"] != 1
                or event.get("event") != "committed_rejection"
                or not isinstance(event.get("hash"), str) or not HEX_32.fullmatch(event["hash"])
                or not isinstance(event.get("reason"), str) or not event["reason"]
                or len(event["reason"].encode()) > 8192 or not isinstance(event.get("stage"), str) or event["stage"] not in STAGES
                or event.get("source") not in {None, "local", "remote", "proposal", "recovery"}
                or (event.get("peer") is not None and not natural(event["peer"]))
                or event.get("resource_observation") != "rejection_preparation"):
            raise ValueError("invalid committed rejection record")
        accounts = event.get("accounts")
        if not isinstance(accounts, list) or len(accounts) > 5:
            raise ValueError("invalid rejection resource observation")
        seen = set()
        for account in accounts:
            if not isinstance(account, dict) or account.get("account") not in ACCOUNT_NAMES:
                raise ValueError("invalid rejection resource account")
            if account["account"] in seen:
                raise ValueError("duplicate rejection resource account")
            seen.add(account["account"])
            for key in ("usage", "limit"):
                amount = account.get(key)
                if not isinstance(amount, dict) or set(amount) != AMOUNT_KEYS or not all(map(natural, amount.values())):
                    raise ValueError("invalid rejection resource amount")
    return events


def validate_success(output: str, *, allow_rejections: bool = False) -> list[dict]:
    events = validate_events(output)
    if events and not allow_rejections:
        raise ValueError("throughput workload emitted a refused candidate or unexpected policy rejection")
    if any(message["level"] == "ERROR" for message in records(output, "BENCH_SERVICE_LOG ")):
        raise ValueError("service reported an error during the workload or shutdown")
    return events


def refusal_hashes(terminals: dict) -> set[str]:
    populations = []
    for name, count in (("accepted_hashes", "accepted"), ("rejected_hashes", "rejected")):
        values = terminals.get(name)
        if (not isinstance(values, list) or any(not isinstance(value, str) or not HEX_32.fullmatch(value) for value in values)
                or len(set(values)) != len(values) or not natural(terminals.get(count)) or terminals[count] != len(values)):
            raise ValueError(f"invalid failure {name}")
        populations.append(set(values))
    expected = populations[1] - populations[0]
    mapped = terminals.get("refused_without_observed_acceptance")
    if not isinstance(mapped, list):
        raise ValueError("failure omitted refusal-to-corpus mapping")
    hashes = [value.get("hash") for value in mapped if isinstance(value, dict)]
    if len(hashes) != len(mapped) or len(set(hashes)) != len(hashes) or set(hashes) != expected:
        raise ValueError("refusal-to-corpus mapping differs from terminal sets")
    for value in mapped:
        if (not natural(value.get("corpus_index")) or value.get("phase") not in {"warm", "target"}
                or (value.get("peer") is not None and not natural(value["peer"]))):
            raise ValueError("refusal lacks an indexed corpus phase or peer")
    return expected


def inspect_failure(output: str) -> dict:
    """Preserve the original failure even when its diagnostics are themselves bad."""
    try:
        events = validate_events(output)
        terminals = one_record(output, "BENCH_FAILURE_TERMINALS ")
        refused = refusal_hashes(terminals)
        diagnosed = {event["hash"] for event in events}
        missing = sorted(refused - diagnosed)
        return {
            "status": "incomplete" if missing else "captured",
            "records": len(events), "refusal_only": len(refused),
            "missing_refusal_hashes": missing,
            "reasons": dict(Counter(event["reason"] for event in events)),
            "stages": dict(Counter(event["stage"] for event in events)),
            "peers": dict(Counter(str(event["peer"]) for event in events)),
            "unresolved": terminals.get("unresolved"),
            "relay_generation_resets": terminals.get("relay_generation_resets"),
            "service_messages": records(output, "BENCH_SERVICE_LOG "),
        }
    except (KeyError, TypeError, ValueError) as error:
        return {"status": "incomplete", "error": str(error)}


def validate_pressure(output: str) -> dict:
    events = validate_success(output, allow_rejections=True)
    pressure = one_record(output, "BENCH_RBF_PRESSURE ")
    corpus = one_record(output, "BENCH_CORPUS ")
    if any(value.startswith("BENCH_RESULT ") for value in output.splitlines()):
        raise ValueError("overload diagnostic emitted a throughput result")
    for key in ("consensus_blake2b", "cycles_blake2b", "transaction_bytes_blake2b", "transaction_hashes_blake2b"):
        if not isinstance(corpus.get(key), str) or not HEX_32.fullmatch(corpus[key]):
            raise ValueError("pressure diagnostic omitted its input identity")
    for key in ("warm", "target", "peers", "refused", "ingress_refused", "after_resume_refused", "target_callbacks_while_paused", "final_live_replacements"):
        if not natural(pressure.get(key)):
            raise ValueError("invalid pressure diagnostic count")
    if (type(pressure.get("schema")) is not int or pressure["schema"] != 1
            or pressure["target"] == 0
            or pressure.get("warm") != pressure["target"]
            or pressure["peers"] == 0 or pressure["ingress_refused"] == 0
            or pressure.get("target_callbacks_while_paused") != 0
            or pressure.get("final_live_replacements") != pressure["target"]
            or corpus.get("transaction_count") != pressure["target"] * 2
            or any(pressure.get(key) is not True for key in (
                "all_refused_victims_retained", "all_retries_accepted", "retry_peer_identity_preserved"))):
        raise ValueError("incomplete RBF pressure or retry contract")
    before = pressure.get("before_retry_terminals", {})
    after = pressure.get("final_terminals", {})
    for terminal in (before, after):
        if (not isinstance(terminal, dict) or terminal.get("planned") != corpus["transaction_count"]
                or terminal.get("relay_ok") != terminal.get("accepted")
                or any(type(terminal.get(key)) is not int or terminal[key] != 0 for key in (
                    "callback_duplicates", "unexpected_callbacks", "relay_duplicate_ok", "relay_duplicate_reject",
                    "relay_generation_resets", "unexpected_rejects", "unresolved"))):
            raise ValueError("pressure diagnostic has duplicate, missing or reset terminals")
    refused = refusal_hashes(before)
    if (len(refused) != pressure["refused"] or len(events) != len(refused)
            or {event["hash"] for event in events} != refused or refusal_hashes(after)
            or after.get("accepted") != corpus["transaction_count"]):
        raise ValueError("RBF refusal causes do not cover the complete settled population")
    mapping = {value["hash"]: value for value in before["refused_without_observed_acceptance"]}
    per_peer_target = (pressure["target"] + pressure["peers"] - 1) // pressure["peers"]
    for event in events:
        item = mapping[event["hash"]]
        peer = event["peer"]
        if (event["reason"] != 'Full("peer pipeline")' or event["stage"] not in {"ingress", "resolve", "verify"}
                or event["source"] != "remote" or peer != item["peer"]
                or not natural(peer) or not 1 <= peer <= pressure["peers"]
                or item["phase"] != "target" or not pressure["warm"] <= item["corpus_index"] < corpus["transaction_count"]
                or peer != (item["corpus_index"] - pressure["warm"]) // per_peer_target + 1
                or not any(account["account"] == "peer" and account["usage"]["items"] > 0 for account in event["accounts"])):
            raise ValueError("RBF refusal lacks its specific policy, stage, peer or quota")
    per_peer = pressure.get("refused_per_peer")
    if (not isinstance(per_peer, list) or not all(map(natural, per_peer))
            or per_peer != [sum(event["peer"] == peer for event in events) for peer in range(1, pressure["peers"] + 1)]
            or pressure["ingress_refused"] + pressure["after_resume_refused"] != len(refused)):
        raise ValueError("pressure refusal phase or peer totals differ")
    return {key: value for key, value in pressure.items() if not key.endswith("terminals")} | {
        "diagnostics": len(events), "reasons": dict(Counter(event["reason"] for event in events)),
        "stages": dict(Counter(event["stage"] for event in events)),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("--pressure", action="store_true", help="require complete RBF overload and retry evidence")
    args = parser.parse_args()
    output = args.log.read_text()
    result = validate_pressure(output) if args.pressure else inspect_failure(output)
    print(json.dumps(result, indent=2, sort_keys=True))
    if not args.pressure and result["status"] != "captured":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
