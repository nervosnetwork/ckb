#!/usr/bin/env python

import os
import sys
import yaml
import json
import psycopg2

conn = psycopg2.connect(
    user     = os.getenv("POSTGRES_USERNAME"),
    password = os.getenv("POSTGRES_PASSWORD"),
    host     = os.getenv("POSTGRES_HOST"),
    port     = os.getenv("POSTGRES_PORT"),
    dbname   = os.getenv("POSTGRES_DATABASE")
)
cur = conn.cursor()

metrics_file = sys.argv[1]
metrics = yaml.load(open(metrics_file))

for metric in metrics:
    cur.execute("""
        INSERT INTO benchmark
        SELECT *
        FROM json_populate_record (NULL::tps_bench, %s);""",
      (json.dumps(metric),)
    )
