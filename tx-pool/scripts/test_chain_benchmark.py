import copy
import json
import tempfile
import unittest
from pathlib import Path

from chain_benchmark import capture, parse


def example(pressure=False):
    count = 1100 if pressure else 8
    hashes = [f"{index:064x}" for index in range(1, count + 1)]
    options = ["recovery-pressure", "1100", "8"] if pressure else ["roundtrip", "8", "1", "8"]
    corpus = {"schema": 1, "options": options, "transactions": count, "workers": 8, "runtime_threads": 16,
              "trusted_synthetic_blocks": True, "profiling": False, "allocation_observation": False,
              "debug_assertions": False, "fixture_blake2b": "f" * 64, "transaction_hashes": hashes}
    stages = ([('detach_recovery_pressure_paused', 0, 550, 0, 1100, 0)] if pressure else [
        ('pending_to_gap', 1, 0, 1, 0, 0), ('gap_to_proposed', 2, 0, 1, 0, 0),
        ('attach_committed_roots', 3, 0, 1, 0, 1), ('detach_roots_with_descendants_paused', 2, 1, 0, 1, 0),
        ('proposed_to_pending', 0, 2, 0, 0, 0)])
    samples = [{"schema": 1, "stage": stage, "tip_number": number, "tip_hash": str(number) * 64,
                "detached_blocks": detached, "attached_blocks": attached, "detached_transactions": dt,
                "attached_transactions": at, "elapsed_ns": 2_000_000, "cpu_ns": 3_000_000,
                "scope": "controller_reconciliation_and_required_publication"}
               for stage, number, detached, attached, dt, at in stages]
    recovery = {"schema": 1, "elapsed_ns": 10_000_000, "scope": "resume_to_observed_empty_recovery_queue"}
    if pressure:
        recovery.update(stage="bounded_recovery_pressure", offered=hashes, retained=hashes[:1000],
                        accepted=hashes[:680], refused=hashes[680:1000], serialized_bytes=178_000_000, serialized_limit=180_000_000)
    else:
        recovery.update(stage="recover_detached_roots_and_descendants", offered=hashes[:4],
                        retained=hashes[:4], accepted=hashes, refused=[])
    records = [("CHAIN_CORPUS", corpus)]
    records.extend(("CHAIN_SAMPLE", sample) for sample in samples[:1 if pressure else 4])
    records.append(("CHAIN_RECOVERY", recovery))
    if not pressure:
        records.append(("CHAIN_SAMPLE", samples[-1]))
    records.append(("CHAIN_COMPLETE", {"schema": 1, "validated": True}))
    return options, records


def log(records):
    return "\n".join(f"{prefix} {json.dumps(value)}" for prefix, value in records) + (
        "\nRESOURCE_RESULT max_rss_bytes=1024 voluntary_context_switches=1 involuntary_context_switches=2\n")


class ChainObservationTests(unittest.TestCase):
    def test_capture_cannot_dirty_the_source_it_measures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            output = root / "capture"
            with self.assertRaises(ValueError):
                capture(root, root / "build.json", None, output, ["roundtrip", "8", "1", "8"], 60)
            self.assertFalse(output.exists())

    def test_roundtrip_and_pressure_have_distinct_completion_contracts(self):
        for pressure in (False, True):
            options, records = example(pressure)
            observation = parse(log(records), options)
            self.assertEqual(len(observation["samples"]), 1 if pressure else 5)
            self.assertEqual(set(observation["stage_qualification"].values()), {"eligible_for_comparison"})

    def test_short_operations_remain_functional_evidence(self):
        options, records = example()
        records[1][1]["elapsed_ns"] = 50_000
        observation = parse(log(records), options)
        self.assertEqual(observation["stage_qualification"]["pending_to_gap"], "functional_only")

    def test_rejects_missing_real_deltas_publication_and_exact_sets(self):
        for mutation in (
            lambda r: r[1][1].update(attached_blocks=0),
            lambda r: r[4][1].update(tip_hash="d" * 64),
            lambda r: r[5][1].update(retained=r[5][1]["retained"][:-1]),
            lambda r: r[5][1].update(accepted=r[5][1]["accepted"][:-1]),
            lambda r: r[5][1].update(offered=r[5][1]["offered"] + ["a" * 64]),
            lambda r: r[-1][1].update(validated=False),
            lambda r: r[0][1].update(profiling=True),
            lambda r: r[2][1].update(scope="enqueue_only"),
        ):
            options, records = copy.deepcopy(example())
            mutation(records)
            with self.assertRaises(ValueError):
                parse(log(records), options)

    def test_pressure_requires_bounded_retention_and_complete_terminal_partition(self):
        for mutation in (
            lambda r: r.update(retained=r["offered"]),
            lambda r: r.update(refused=r["refused"][:-1]),
            lambda r: r.update(refused=r["refused"] + [r["accepted"][0]]),
            lambda r: r.update(serialized_bytes=r["serialized_limit"] + 1),
        ):
            options, records = copy.deepcopy(example(True))
            mutation(records[2][1])
            with self.assertRaises(ValueError):
                parse(log(records), options)


if __name__ == "__main__":
    unittest.main()
