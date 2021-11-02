#!/usr/bin/env python3

# Usage: `./process-metrics.py metrics.json`

import os
import sys
import json
import yaml
from urllib.parse import urlparse
import psycopg2
from psycopg2 import sql

INSTANCE_TYPE = os.getenv("INSTANCE_TYPE")
INSTANCE_BASTION_TYPE = os.getenv("INSTANCE_BASTION_TYPE")
BENCHMARK_POSTGRES_SECRET = os.getenv("secrets")


def rewrite_metric(metric):
    # Construct new metric
    metric["instance_type"] = INSTANCE_TYPE
    metric["instance_bastion_type"] = INSTANCE_BASTION_TYPE
    return metric


def update_benchmark_to_database(metric) -> bool:
    """
    insert benchmark result into ckb analyzer database
    :param metric: metric as saved report information
    :return: None
    """
    o = urlparse(BENCHMARK_POSTGRES_SECRET['BENCH_DB_CONN'], 'postgres')
    user = o.username.strip()
    password = o.password.strip()
    host = o.hostname.strip()
    port = o.port
    db = o.path.strip('/')

    if not user or not password or not host or port is None or not db
        print("urlparse error, please check secrets")
        return False

    result = False
    conn = None
    try:
        conn = psycopg2.connect(
            host=host,
            database=db,
            user=user,
            password=password)
        cur = conn.cursor()
        cur.execute(
            sql.SQL(
                """insert into ci_bench (time, average_block_time_ms, average_block_transactions,
                average_block_transactions_size, version, delay_time_ms,
                from_block_number, instance_bastion_type, instance_type,
                n_inout, n_nodes, to_block_number,
                total_transactions, total_transactions_size, transactions_per_second,
                transactions_size_per_second) values (%s, %s, %s)"""),
            [metric["time"], metric["average_block_time_ms"], metric["average_block_transactions"],
             metric["average_block_transactions_size"], metric["version"], metric["delay_time_ms"],
             metric["from_block_number"], metric["instance_bastion_type"], metric["instance_type"], metric["n_inout"],
             metric["n_nodes"], metric["to_block_number"], metric["total_transactions"],
             metric["total_transactions_size"], metric["transactions_per_second"],
             metric["transactions_size_per_second"]])

    except (Exception, psycopg2.DatabaseError) as error:
        print(error)
    else:
        result = true
    finally:
        if conn is not None:
            conn.close()

    return result


def main():
    metrics = []
    for line in open(sys.argv[1]):
        metric = json.loads(line)
        metric = rewrite_metric(metric)
        metrics.append(metric)

    print(yaml.dump(metrics))
    update_benchmark_to_database(metric)


main()
