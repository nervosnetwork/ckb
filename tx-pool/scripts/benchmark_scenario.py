"""Shared CLI gates for the finite executor's fixture and submission shapes.

The native fixture producer separately derives DAO and encoding capacity from
its actual system cells before allocating the funding population.
"""

import re
import sys

MAX_TRANSACTIONS = 65_536
FANOUT_COHORT_SIZE = 65
TRANSACTION_SIZE_LIMIT = 512_000
# build_success_tx's fixed Molecule shape; the native preflight checks it against
# real zero/one-input or zero/one-output fixtures before the large allocation.
INPUT_BYTES, OUTPUT_BYTES = 44, 89
FANIN_BASE_BYTES, FANOUT_BASE_BYTES = 198, 153


def suffix_number(value: str, label: str, maximum: int) -> int:
    if re.fullmatch(r"\+?[0-9]+", value) is None or int(value) > maximum:
        raise ValueError(f"invalid {label}: {value}")
    return int(value)


def validate_scenario(name: str, target: int, warm: int, workers: int, peers: int) -> None:
    usize_max = sys.maxsize * 2 + 1
    if (not isinstance(name, str) or not name
            or any(type(value) is not int or not 0 <= value <= usize_max
                   for value in (target, warm, workers, peers))
            or not target or not workers or not peers):
        raise ValueError("target, workers and peers must be positive; warm must be nonnegative")
    population = target + warm
    if population > MAX_TRANSACTIONS:
        raise ValueError("target + warm must be at most 65536")
    if name.startswith("dependent_forest_"):
        spec = name.removeprefix("dependent_forest_")
        depth = suffix_number(spec.removesuffix("_reverse"), "dependency depth", usize_max)
        if depth == 0:
            raise ValueError("dependency depth must be non-zero")
        if not spec.endswith("_reverse") and (target % depth or warm % depth):
            raise ValueError("forward forest target and warm must each contain complete chains")
    elif name.startswith("always_success_fanin_"):
        fan_in = suffix_number(name.removeprefix("always_success_fanin_"), "fan-in", usize_max)
        if fan_in == 0:
            raise ValueError("fan-in must be non-zero")
        if fan_in > (TRANSACTION_SIZE_LIMIT - FANIN_BASE_BYTES) // INPUT_BYTES:
            raise ValueError("fan-in transaction exceeds transaction size limit")
    elif name.startswith("always_success_callback_") and name.endswith("us"):
        suffix_number(name.removeprefix("always_success_callback_").removesuffix("us"),
                      "callback delay", 2**64 - 1)
    elif name not in ("always_success", "secp256k1", "dependent", "dependent_reverse", "fanout",
                      "fanout_reverse", "fanout_ready_64_reverse", "rbf_pairs", "rbf_pairs_windowed",
                      "rbf_pressure", "reorg_in_flight"):
        raise ValueError(f"unknown scenario: {name}")
    if name in ("rbf_pairs", "rbf_pairs_windowed", "rbf_pressure") and target != warm:
        raise ValueError("RBF workload requires equal warm and target counts")
    if name.endswith("_reverse") and name != "fanout_ready_64_reverse" and warm:
        raise ValueError("reverse dependency workloads require warm=0")
    if name == "fanout_ready_64_reverse" and (target % FANOUT_COHORT_SIZE or warm % FANOUT_COHORT_SIZE):
        raise ValueError("fanout cohort target and warm counts must each be multiples of 65")
    if name in ("fanout", "fanout_reverse"):
        if population < 2:
            raise ValueError("fanout requires a parent and a child")
        if FANOUT_BASE_BYTES + OUTPUT_BYTES * (population - 1) > TRANSACTION_SIZE_LIMIT:
            raise ValueError("fanout parent exceeds transaction size limit")
