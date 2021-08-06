#!/usr/bin/env python3

# Usage: `./process-metrics.py metrics.json`

import os
import sys
import json
import yaml

INSTANCE_TYPE = os.getenv("INSTANCE_TYPE")
INSTANCE_BASTION_TYPE = os.getenv("INSTANCE_BASTION_TYPE")


def rewrite_metric(metric):
    # Construct new metric
    metric["instance_type"] = INSTANCE_TYPE
    metric["instance_bastion_type"] = INSTANCE_BASTION_TYPE
    return metric


def main():
    metrics = []
    for line in open(sys.argv[1]):
        metric = json.loads(line)
        metric = rewrite_metric(metric)
        metrics.append(metric)

    print(yaml.dump(metrics))


main()
