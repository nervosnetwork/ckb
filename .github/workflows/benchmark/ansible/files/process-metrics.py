#!/usr/bin/env python3

import os
import sys
import json
import yaml

INSTANCE_TYPE = os.getenv("INSTANCE_TYPE")
INSTANCE_BASTION_TYPE = os.getenv("INSTANCE_BASTION_TYPE")


def rewrite_metric(metric):
    # Construct new metric
    metric = {
        "time"                   : metric["time"],
        "send_delay"             : metric["send_delay"],
        "tps"                    : metric["tps"],
        "ckb_version"            : metric["ckb_version"],
        "transaction_type"       : metric["transaction_type"],
        "instances_count"        : metric["instances_count"],
        "total_transactions_size": metric["total_transactions_size"],
        "instance_type"          : INSTANCE_TYPE,
        "instance_bastion_type"  : INSTANCE_BASTION_TYPE
    }
    return metric


def main():
    metrics = []
    for line in open(sys.argv[1]):
        metric = json.loads(line)
        metric = rewrite_metric(metric)
        metrics.append(metric)

    print(yaml.dump(metrics))


main()
