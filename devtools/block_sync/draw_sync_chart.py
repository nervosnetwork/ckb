#!/usr/bin/env python3
import matplotlib.pyplot as plt
import re
import datetime
import tqdm
import argparse

from matplotlib.ticker import MultipleLocator


def parse_sync_statics(log_file):
    """
    parse sync statics from log file
    sample:
    2023-09-01 06:54:45.096 +00:00 verify_blocks INFO ckb_chain::chain  block: 811224, hash: 0x00f54aaadd1a36339e69a10624dec3250658100ffd5773a7e9f228bb9a96187e, epoch: 514(841/1800), total_diff: 0x59a4a071ba9f0de59d, txs: 1
    """
    duration = []
    height = []
    base_timestamp = 0

    print("reading file: ", log_file)
    total_lines = len(open(log_file, 'r').readlines())
    print("total lines: ", total_lines)

    with open(log_file, 'r') as f:
        pbar = tqdm.tqdm(total=total_lines)
        for line_idx, line in enumerate(f):
            pbar.update(1)
            if line.find('INFO ckb_chain::chain  block: ') != -1:
                timestamp_str = re.search(r'^(\S+ \S+)', line).group(1)  # Extract the timestamp string
                timestamp = datetime.datetime.strptime(timestamp_str, "%Y-%m-%d %H:%M:%S.%f").timestamp()

                if base_timestamp == 0:
                    base_timestamp = timestamp
                timestamp = int(timestamp - base_timestamp)

                block_number = int(re.search(r'block: (\d+)', line).group(1))  # Extract the block number using regex

                if line_idx == 0 or block_number % 10000 == 0:
                    duration.append(timestamp / 60 / 60)
                    height.append(block_number)

        pbar.close()

    return duration, height


parser = argparse.ArgumentParser(
    description='Draw CKB Sync progress Chart. Usage: ./draw_sync_chart.py --ckb_log ./run1.log ./run2.log --label branch_develop branch_async --result_path /tmp/compare_result.png')
parser.add_argument('--ckb_log', metavar='ckb_log_file', type=str,
                    action='store', nargs='+', required=True,
                    help='the ckb node log file path')
parser.add_argument('--label', metavar='label', type=str,
                    action='store', nargs='+', required=True,
                    help='what label should be put on the chart')
parser.add_argument('--result_path', type=str, nargs=1, action='store',
                    help='where to save the result chart')

args = parser.parse_args()
assert len(args.ckb_log) == len(args.label)

tasks = zip(args.ckb_log, args.label)

result_path = args.result_path[0]
fig, ax = plt.subplots(1, 1, figsize=(10, 8))

lgs = []
for ckb_log_file, label in tasks:
    print("ckb_log_file: ", ckb_log_file)
    print("label: ", label)
    duration, height = parse_sync_statics(ckb_log_file)

    lg = ax.scatter(duration, height, s=1, label=label)
    ax.plot(duration, height, label=label)

    lgs.append(lg)

    for i, h in enumerate(height):
        if h % 2000000 == 0:
            ax.vlines([duration[i]], 0, h, colors="gray", linestyles="dashed")

    ax.get_yaxis().get_major_formatter().set_scientific(False)
    ax.get_yaxis().get_major_formatter().set_useOffset(False)
    
    ax.margins(0)

    ax.set_axisbelow(True)

    ax.xaxis.grid(color='gray', linestyle='solid', which='major')
    ax.yaxis.grid(color='gray', linestyle='solid', which='major')

    ax.xaxis.grid(color='gray', linestyle='dashed', which='minor')
    ax.yaxis.grid(color='gray', linestyle='dashed', which='minor')
    
    minorLocator = MultipleLocator(10)
    ax.xaxis.set_minor_locator(minorLocator)
    
    plt.setp(ax.get_xticklabels(), rotation=30, horizontalalignment='right')

plt.legend(tuple(lgs), tuple(args.label), loc='upper left', shadow=True)
plt.title('CKB Block Sync progress Chart')
plt.xlabel('Timecost (hours)')
plt.ylabel('Block Height')
plt.savefig(result_path)
