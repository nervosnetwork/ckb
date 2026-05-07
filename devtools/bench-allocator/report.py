#!/usr/bin/env python3
import argparse
import csv
import statistics
from pathlib import Path


FIELDS = [
    "allocator",
    "run",
    "txs_size",
    "rounds",
    "elapsed_ms",
    "before_rss_bytes",
    "after_setup_rss_bytes",
    "after_workload_rss_bytes",
    "after_drop_rss_bytes",
    "peak_rss_bytes",
    "virtual_memory_bytes",
    "time_elapsed_seconds",
    "time_user_seconds",
    "time_system_seconds",
    "time_max_rss_kb",
    "time_minor_faults",
    "time_major_faults",
]


SUMMARY_METRICS = [
    ("elapsed_ms", "elapsed_s", 1000.0),
    ("after_workload_rss_bytes", "workload_rss_mib", 1024.0 * 1024.0),
    ("after_drop_rss_bytes", "drop_rss_mib", 1024.0 * 1024.0),
    ("peak_rss_bytes", "peak_rss_mib", 1024.0 * 1024.0),
    ("virtual_memory_bytes", "vms_mib", 1024.0 * 1024.0),
    ("time_elapsed_seconds", "time_elapsed_s", 1.0),
    ("time_user_seconds", "time_user_s", 1.0),
    ("time_system_seconds", "time_system_s", 1.0),
    ("time_max_rss_kb", "time_max_rss_mib", 1024.0),
    ("time_minor_faults", "minor_faults", 1.0),
    ("time_major_faults", "major_faults", 1.0),
]


def parse_elapsed(value):
    parts = value.strip().split(":")
    if len(parts) == 2:
        minutes, seconds = parts
        return int(minutes) * 60 + float(seconds)
    if len(parts) == 3:
        hours, minutes, seconds = parts
        return int(hours) * 3600 + int(minutes) * 60 + float(seconds)
    return float(value)


def read_time_log(path):
    data = {}
    for line in Path(path).read_text().splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        key = key.strip()
        value = value.strip()
        if key == "Elapsed (wall clock) time (h:mm:ss or m:ss)":
            data["time_elapsed_seconds"] = parse_elapsed(value)
        elif key == "User time (seconds)":
            data["time_user_seconds"] = float(value)
        elif key == "System time (seconds)":
            data["time_system_seconds"] = float(value)
        elif key == "Maximum resident set size (kbytes)":
            data["time_max_rss_kb"] = int(value)
        elif key == "Minor (reclaiming a frame) page faults":
            data["time_minor_faults"] = int(value)
        elif key == "Major (requiring I/O) page faults":
            data["time_major_faults"] = int(value)
    return data


def read_result_csv_line(path):
    rows = [
        line.strip().split(",")
        for line in Path(path).read_text().splitlines()
        if line.startswith("result_csv,") and not line.startswith("result_csv,allocator,")
    ]
    if not rows:
        raise SystemExit(f"missing result_csv line in {path}")
    row = rows[-1]
    keys = [
        "marker",
        "allocator",
        "txs_size",
        "rounds",
        "elapsed_ms",
        "before_rss_bytes",
        "after_setup_rss_bytes",
        "after_workload_rss_bytes",
        "after_drop_rss_bytes",
        "peak_rss_bytes",
        "virtual_memory_bytes",
    ]
    return dict(zip(keys, row))


def append(args):
    row = read_result_csv_line(args.log)
    row.update(read_time_log(args.time_log))
    row["allocator"] = args.allocator
    row["run"] = args.run

    csv_path = Path(args.csv)
    exists = csv_path.exists()
    with csv_path.open("a", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=FIELDS)
        if not exists:
            writer.writeheader()
        writer.writerow({field: row.get(field, "") for field in FIELDS})


def as_number(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def metric_values(rows, allocator, metric):
    values = [
        as_number(row.get(metric))
        for row in rows
        if row.get("allocator") == allocator and row.get(metric)
    ]
    return [value for value in values if value is not None]


def scaled_metric_values(rows, allocator, metric, scale):
    return [value / scale for value in metric_values(rows, allocator, metric)]


def metric_stats(rows, allocator, metric, scale=1.0):
    values = scaled_metric_values(rows, allocator, metric, scale)
    if not values:
        return None
    stdev = statistics.stdev(values) if len(values) > 1 else 0.0
    return {
        "count": len(values),
        "mean": statistics.mean(values),
        "median": statistics.median(values),
        "stdev": stdev,
        "min": min(values),
        "max": max(values),
    }


def paired_deltas(rows, metric, scale):
    by_run = {}
    for row in rows:
        allocator = row.get("allocator")
        run = row.get("run")
        value = as_number(row.get(metric))
        if allocator not in {"jemalloc", "mimalloc"} or run is None or value is None:
            continue
        by_run.setdefault(run, {})[allocator] = value / scale

    deltas = []
    for run, values in by_run.items():
        if "jemalloc" in values and "mimalloc" in values:
            deltas.append(values["mimalloc"] - values["jemalloc"])
    return deltas


def fmt(value):
    if abs(value) >= 1000:
        return f"{value:,.1f}"
    return f"{value:.2f}"


def summarize(args):
    rows = list(csv.DictReader(Path(args.csv).open()))
    allocators = sorted({row["allocator"] for row in rows})
    lines = ["# Allocator Benchmark Summary", ""]
    lines.append(f"Rows: {len(rows)}")
    for key in ("txs_size", "rounds"):
        values = sorted({row.get(key, "") for row in rows if row.get(key)})
        if len(values) == 1:
            lines.append(f"{key}: {values[0]}")
    lines.append("")
    lines.append("## Summary")
    lines.append("")
    lines.append("| Allocator | Metric | Mean | Median | Stddev | Min | Max |")
    lines.append("| --- | --- | ---: | ---: | ---: | ---: | ---: |")
    for allocator in allocators:
        for metric, label, scale in SUMMARY_METRICS:
            stats = metric_stats(rows, allocator, metric, scale)
            if stats is None:
                continue
            lines.append(
                f"| {allocator} | {label} | {fmt(stats['mean'])} | "
                f"{fmt(stats['median'])} | {fmt(stats['stdev'])} | "
                f"{fmt(stats['min'])} | {fmt(stats['max'])} |"
            )
    if {"jemalloc", "mimalloc"}.issubset(set(allocators)):
        lines.append("")
        lines.append("## Mimalloc Delta")
        lines.append("")
        lines.append(
            "| Metric | Jemalloc Mean | Mimalloc Mean | Delta | Delta % | Paired Median Delta |"
        )
        lines.append("| --- | ---: | ---: | ---: | ---: | ---: |")
        for metric, label, scale in SUMMARY_METRICS:
            jemalloc = metric_stats(rows, "jemalloc", metric, scale)
            mimalloc = metric_stats(rows, "mimalloc", metric, scale)
            if jemalloc is None or mimalloc is None or jemalloc["mean"] == 0:
                continue
            delta = mimalloc["mean"] - jemalloc["mean"]
            delta_percent = delta / jemalloc["mean"] * 100
            pair_values = paired_deltas(rows, metric, scale)
            pair_median = statistics.median(pair_values) if pair_values else None
            lines.append(
                f"| {label} | {fmt(jemalloc['mean'])} | {fmt(mimalloc['mean'])} | "
                f"{fmt(delta)} | {delta_percent:.2f}% | "
                f"{fmt(pair_median) if pair_median is not None else ''} |"
            )
    Path(args.output).write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(required=True)

    append_parser = subparsers.add_parser("append")
    append_parser.add_argument("--allocator", required=True)
    append_parser.add_argument("--run", required=True)
    append_parser.add_argument("--log", required=True)
    append_parser.add_argument("--time-log", required=True)
    append_parser.add_argument("--csv", required=True)
    append_parser.set_defaults(func=append)

    summarize_parser = subparsers.add_parser("summarize")
    summarize_parser.add_argument("--csv", required=True)
    summarize_parser.add_argument("--output", required=True)
    summarize_parser.set_defaults(func=summarize)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
