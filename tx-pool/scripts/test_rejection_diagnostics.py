"""Failure-evidence gates, including the runner's actual child-process path."""

from copy import deepcopy
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cross_version_benchmark as benchmark
import rejection_diagnostics as diagnostics


def fixture():
    warm = ["00" * 32, "11" * 32]
    target = ["22" * 32, "33" * 32]

    def terminals(accepted, rejected, refused):
        return dict(
            planned=4, accepted=len(accepted), accepted_hashes=accepted,
            rejected=len(rejected), rejected_hashes=rejected, relay_ok=len(accepted),
            callback_duplicates=0, unexpected_callbacks=0, relay_duplicate_ok=0,
            relay_duplicate_reject=0, relay_generation_resets=0, unexpected_rejects=0,
            unresolved=0, refused_without_observed_acceptance=refused,
        )

    mapping = dict(hash=target[1], corpus_index=3, phase="target", peer=2)
    amount = dict(items=1, bytes=100, edges=2, serialized=0, cycles=0)
    event = dict(schema=1, event="committed_rejection", hash=target[1], reason='Full("peer pipeline")',
                 stage="ingress", source="remote", peer=2, resource_observation="rejection_preparation",
                 accounts=[dict(account="peer", usage=amount, limit=amount | {"bytes": 200})])
    before = terminals(warm + [target[0]], [warm[0], target[1]], [mapping])
    after = terminals(warm + target, warm + [target[1]], [])
    pressure = dict(schema=1, warm=2, target=2, peers=2, refused=1, ingress_refused=1,
                    after_resume_refused=0, refused_per_peer=[0, 1], target_callbacks_while_paused=0,
                    all_refused_victims_retained=True, all_retries_accepted=True,
                    retry_peer_identity_preserved=True, final_live_replacements=2,
                    before_retry_terminals=before, final_terminals=after)
    corpus = dict(transaction_count=4, consensus_blake2b="44" * 32, cycles_blake2b="55" * 32,
                  transaction_bytes_blake2b="66" * 32, transaction_hashes_blake2b="77" * 32)
    capture = dict(schema=1, logger="rejections_and_warnings_v1", records=1, service_records=0, write_failed=False)
    return [
        ("BENCH_CORPUS ", corpus), ("BENCH_REJECTION ", event),
        ("BENCH_RBF_PRESSURE ", pressure), ("BENCH_REJECTION_CAPTURE ", capture),
    ]


def encode(rows):
    return "".join(prefix + json.dumps(value) + "\n" for prefix, value in rows)


class RejectionEvidenceTests(unittest.TestCase):
    def test_complete_pressure_capture_and_required_negative_evidence(self):
        rows = fixture()
        result = diagnostics.validate_pressure(encode(rows))
        self.assertEqual(result["diagnostics"], 1)
        self.assertEqual(result["reasons"], {'Full("peer pipeline")': 1})
        cases = []
        for index in (0, 1, 2, 3):
            cases.append(rows[:index] + rows[index + 1:])
        for key, value in (("event", "prepared_rejection"), ("hash", "88" * 32),
                           ("reason", "unknown"), ("stage", "unknown"), ("peer", 1),
                           ("accounts", [])):
            changed = deepcopy(rows)
            changed[1][1][key] = value
            cases.append(changed)
        for key, value in (("records", 0), ("records", True), ("write_failed", True)):
            changed = deepcopy(rows)
            changed[3][1][key] = value
            cases.append(changed)
        for key, value in (("all_refused_victims_retained", False), ("refused_per_peer", [1, 0]),
                           ("after_resume_refused", 1), ("target_callbacks_while_paused", 1)):
            changed = deepcopy(rows)
            changed[2][1][key] = value
            cases.append(changed)
        changed = deepcopy(rows)
        changed[2][1]["before_retry_terminals"]["refused_without_observed_acceptance"] = []
        cases.append(changed)
        changed = deepcopy(rows)
        changed[2][1]["final_terminals"]["relay_generation_resets"] = 1
        cases.append(changed)
        changed = deepcopy(rows)
        changed[2][1].update(ingress_refused=0, after_resume_refused=1)
        cases.append(changed)
        changed = deepcopy(rows)
        changed[2][1]["before_retry_terminals"]["refused_without_observed_acceptance"][0]["corpus_index"] = 2
        cases.append(changed)
        changed = deepcopy(rows)
        changed[3][1]["service_records"] = 1
        changed.append(("BENCH_SERVICE_LOG ", dict(level="ERROR", target="ckb_tx_pool", message="save failed")))
        cases.append(changed)
        for index, changed in enumerate(cases):
            with self.subTest(index=index), self.assertRaises(ValueError):
                diagnostics.validate_pressure(encode(changed))
        with self.assertRaisesRegex(ValueError, "throughput"):
            diagnostics.validate_pressure(encode(rows) + "BENCH_RESULT falsely_successful\n")

    def test_success_requires_complete_capture_and_successful_service_shutdown(self):
        capture = fixture()[-1][1] | {"records": 0}
        valid = encode([("BENCH_REJECTION_CAPTURE ", capture)])
        self.assertEqual(diagnostics.validate_success(valid), [])
        warning = ("BENCH_SERVICE_LOG ", dict(level="WARN", target="ckb_tx_pool", message="database disabled"))
        self.assertEqual(diagnostics.validate_success(encode([
            warning, ("BENCH_REJECTION_CAPTURE ", capture | {"service_records": 1}),
        ])), [])
        error = ("BENCH_SERVICE_LOG ", warning[1] | {"level": "ERROR"})
        with self.assertRaisesRegex(ValueError, "service reported"):
            diagnostics.validate_success(encode([
                error, ("BENCH_REJECTION_CAPTURE ", capture | {"service_records": 1}),
            ]))
        with self.assertRaisesRegex(ValueError, "capture is incomplete"):
            diagnostics.validate_success(valid + encode([error]))

    def test_failed_process_retains_original_output_and_diagnostic_gaps(self):
        rows = fixture()
        before = rows[2][1]["before_retry_terminals"]
        output = encode(rows[:2] + [("BENCH_FAILURE_TERMINALS ", before), rows[3]])
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            binary = root / "failed-workload"
            for captured in (True, False):
                text = output if captured else output.replace(encode([rows[1]]), "")
                binary.write_text(f"#!{sys.executable}\nprint({text!r})\nraise SystemExit(7)\n")
                binary.chmod(0o700)
                attempt = benchmark.run_attempt(
                    benchmark.binary_record(binary), root, benchmark.parse_scenario("rbf_pairs,2,2,1,2"),
                    "baseline", "failure-capture", 5, "disabled",
                )
                self.assertEqual(attempt["outcome"], "failure")
                self.assertEqual(attempt["category"], "nonzero_exit")
                self.assertIn("status 7", attempt["detail"])
                self.assertIn(text, attempt["output"])
                evidence = attempt["rejection_diagnostics"]
                self.assertEqual(evidence["status"], "captured" if captured else "incomplete")
                if captured:
                    self.assertEqual(evidence["reasons"], {'Full("peer pipeline")': 1})
                    self.assertEqual(evidence["missing_refusal_hashes"], [])


if __name__ == "__main__":
    unittest.main()
