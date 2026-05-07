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


def metric_stats(rows, allocator, metric):
    values = metric_values(rows, allocator, metric)
    if not values:
        return None
    stdev = statistics.stdev(values) if len(values) > 1 else 0.0
    return {
        "count": len(values),
        "mean": statistics.mean(values),
        "stdev": stdev,
        "min": min(values),
        "max": max(values),
    }


def summarize(args):
    rows = list(csv.DictReader(Path(args.csv).open()))
    metrics = [
        "elapsed_ms",
        "after_workload_rss_bytes",
        "after_drop_rss_bytes",
        "peak_rss_bytes",
        "time_elapsed_seconds",
        "time_user_seconds",
        "time_system_seconds",
        "time_max_rss_kb",
        "time_minor_faults",
        "time_major_faults",
    ]
    allocators = sorted({row["allocator"] for row in rows})
    lines = ["# Allocator Benchmark Summary", ""]
    lines.append(f"runs={len(rows)}")
    lines.append("")
    lines.append("| allocator | metric | mean | stdev | min | max |")
    lines.append("| --- | --- | ---: | ---: | ---: | ---: |")
    for allocator in allocators:
        for metric in metrics:
            stats = metric_stats(rows, allocator, metric)
            if stats is None:
                continue
            lines.append(
                f"| {allocator} | {metric} | {stats['mean']:.2f} | "
                f"{stats['stdev']:.2f} | {stats['min']:.2f} | {stats['max']:.2f} |"
            )
    if {"jemalloc", "mimalloc"}.issubset(set(allocators)):
        lines.append("")
        lines.append("## Mimalloc Delta")
        lines.append("")
        lines.append("| metric | jemalloc mean | mimalloc mean | delta | delta % |")
        lines.append("| --- | ---: | ---: | ---: | ---: |")
        for metric in metrics:
            jemalloc = metric_stats(rows, "jemalloc", metric)
            mimalloc = metric_stats(rows, "mimalloc", metric)
            if jemalloc is None or mimalloc is None or jemalloc["mean"] == 0:
                continue
            delta = mimalloc["mean"] - jemalloc["mean"]
            delta_percent = delta / jemalloc["mean"] * 100
            lines.append(
                f"| {metric} | {jemalloc['mean']:.2f} | {mimalloc['mean']:.2f} | "
                f"{delta:.2f} | {delta_percent:.2f}% |"
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
