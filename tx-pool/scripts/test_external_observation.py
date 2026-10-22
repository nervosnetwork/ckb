import copy
import json
import unittest

from external_observation import parse as parse_observation

LOAD = {"concurrency": 100, "poll_ms": 5, "timeout_s": 60}


def parse(output, metadata, hashes):
    return parse_observation(output, metadata, hashes, load=LOAD)


def example():
    hashes = ["0x" + "11" * 32, "0x" + "22" * 32]
    metadata = {"schema": 1, "kind": "fixed_secp_corpus", "count": 2, "inputs": 1,
                "tip_number": 48, "tip_hash": "0x" + "33" * 32, "transactions_blake2b": "44" * 32}
    sends = [{"index": i, "hash": transaction, "attempt": 0, "submitted_ns": 10 + i,
              "rpc_completed_ns": 30 + i, "response": {"id": 1, "jsonrpc": "2.0", "result": transaction}}
             for i, transaction in enumerate(hashes)]
    blocks = [{"number": 49, "hash": "0x" + "55" * 32, "parent_hash": metadata["tip_hash"],
               "last_negative_tip_start_ns": 900_000_000, "tip_rpc_start_ns": 1_000_000_000,
               "tip_rpc_end_ns": 1_010_000_000, "block_rpc_start_ns": 1_020_000_000,
               "first_positive_block_end_ns": 1_100_000_000, "transactions": hashes.copy()}]
    result = {"schema": 1, "outcome": "success", "error": None, "corpus": metadata,
              "automatic_retries": 0, "clock": "process_monotonic",
              "confirmation": "first_observed_canonical_block_with_last_negative_bound",
              "build": {"profiling": False, "allocation_observation": False, "debug_assertions": False},
              "elapsed_ns": 1_200_000_000, **LOAD}
    return metadata, hashes, sends, blocks, result


def log(sends, blocks, result):
    records = [("EXTERNAL_SEND", item) for item in sends] + [("EXTERNAL_BLOCK", item) for item in blocks]
    records.append(("EXTERNAL_RESULT", result))
    return "EXTERNAL_READY\n" + "\n".join(f"{prefix} {json.dumps(item)}" for prefix, item in records)


class ObservationTests(unittest.TestCase):
    def test_reports_observed_completion_and_conservative_bounds(self):
        metadata, hashes, sends, blocks, result = example()
        observed = parse(log(sends, blocks, result), metadata, hashes)
        self.assertEqual(observed["unique_confirmed"], 2)
        self.assertEqual(observed["completion_duration_bounds_ns"], [899_999_990, 1_099_999_990])
        self.assertEqual(observed["confirmation_latency_bounds_ns"]["p99"], [899_999_990, 1_099_999_990])
        self.assertEqual(observed["completion_observation_uncertainty_ns"], 200_000_000)
        self.assertGreater(observed["rpc_completion_tps"], observed["observed_confirmation_tps"])
        self.assertEqual(observed["timing_qualification"], "eligible_for_comparison")

    def test_confirmation_can_precede_rpc_response(self):
        metadata, hashes, sends, blocks, result = example()
        sends[0]["rpc_completed_ns"] = 1_150_000_000
        parse(log(sends, blocks, result), metadata, hashes)

    def test_bounds_are_clamped_to_submission(self):
        metadata, hashes, sends, blocks, result = example()
        blocks[0]["last_negative_tip_start_ns"] = 0
        observed = parse(log(sends, blocks, result), metadata, hashes)
        self.assertEqual(observed["confirmation_latency_bounds_ns"]["p99"][0], 0)

    def test_rejects_invalid_membership_clocks_and_producer_outcomes(self):
        mutations = {
            "refusal": lambda s, b, r: s[0].update(response={"id": 1, "jsonrpc": "2.0", "error": {"code": -1}}),
            "wrong_rpc_hash": lambda s, b, r: s[0]["response"].update(result=s[1]["hash"]),
            "duplicate_submit": lambda s, b, r: s[1].update(hash=s[0]["hash"]),
            "retry": lambda s, b, r: s[0].update(attempt=1),
            "duplicate_confirmation": lambda s, b, r: b[0]["transactions"].append(s[0]["hash"]),
            "missing_confirmation": lambda s, b, r: b[0]["transactions"].pop(),
            "unknown_confirmation": lambda s, b, r: b[0]["transactions"].append("0x" + "ff" * 32),
            "fork": lambda s, b, r: b[0].update(parent_hash="0x" + "ff" * 32),
            "missing_height": lambda s, b, r: b[0].update(number=50),
            "reverse_clock": lambda s, b, r: s[0].update(rpc_completed_ns=0),
            "invalid_bound": lambda s, b, r: b[0].update(last_negative_tip_start_ns=1_050_000_000),
            "late_observation": lambda s, b, r: b[0].update(first_positive_block_end_ns=r["elapsed_ns"] + 1),
            "failed_native": lambda s, b, r: r.update(outcome="failure"),
            "instrumentation": lambda s, b, r: r["build"].update(profiling=True),
            "different_load": lambda s, b, r: r.update(concurrency=99),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                metadata, hashes, sends, blocks, result = copy.deepcopy(example())
                mutate(sends, blocks, result)
                with self.assertRaises(ValueError):
                    parse(log(sends, blocks, result), metadata, hashes)

    def test_rejects_changed_seed_and_incomplete_records(self):
        metadata, hashes, sends, blocks, result = example()
        output = log(sends, blocks, result)
        wrong = metadata | {"tip_hash": "0x" + "ff" * 32}
        for raw, expected in ((output, wrong), (output.replace("EXTERNAL_READY\n", ""), metadata),
                              (output + "\n" + output, metadata), (log(sends[:-1], blocks, result), metadata)):
            with self.assertRaises(ValueError):
                parse(raw, expected, hashes)


if __name__ == "__main__":
    unittest.main()
