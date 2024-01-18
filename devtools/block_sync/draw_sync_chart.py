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
        # pbar = tqdm.tqdm(total=total_lines)
        for line_idx, line in enumerate(f):
            # pbar.update(1)
            if line_idx == 0:
                timestamp_str = re.search(r'^(\S+ \S+)', line).group(1)  # Extract the timestamp string
                timestamp = datetime.datetime.strptime(timestamp_str, "%Y-%m-%d %H:%M:%S.%f").timestamp()
                base_timestamp = timestamp
             
          
            if line.find('INFO ckb_chain::chain  block: ') != -1:

                block_number = int(re.search(r'block: (\d+)', line).group(1))  # Extract the block number using regex

                if line_idx == 0 or block_number % 10_000 == 0:
                    timestamp_str = re.search(r'^(\S+ \S+)', line).group(1)  # Extract the timestamp string
                    timestamp = datetime.datetime.strptime(timestamp_str, "%Y-%m-%d %H:%M:%S.%f").timestamp()
                    timestamp = int(timestamp - base_timestamp)
                    duration.append(timestamp / 60 / 60)
                    height.append(block_number)

        # pbar.close()

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

def process_task(task):
    ckb_log_file, label = task
    print("ckb_log_file: ", ckb_log_file)
    print("label: ", label)
    duration, height = parse_sync_statics(ckb_log_file)
    return (duration, height, label)


tasks = [(ckb_log_file, label) for ckb_log_file, label in tasks]
  

import multiprocessing
with multiprocessing.Pool() as pool:
    results = pool.map(process_task, tasks)

alabels = []

import matplotlib.ticker as ticker

for duration, height, label in results:
# for ckb_log_file, label in tasks:
#     print("ckb_log_file: ", ckb_log_file)
#     print("label: ", label)
#     duration, height = parse_sync_statics(ckb_log_file)


    lg = ax.scatter(duration, height, s=1, label=label)
    ax.plot(duration, height, label=label)


    lgs.append(lg)

    ax.hlines([11_500_000], 0, max(duration), colors="gray", linestyles="dashed")

    for i, h in enumerate(height):
        if h % 1_000_000 == 0:
            ax.vlines([duration[i]], 0, h, colors="gray", linestyles="dashed")

        if i == len(height) -1 :
            alabels.append(((duration[i],h),label))

        if h == 11_000_000 or h == 11_500_000:
            ax.vlines([duration[i]], 0, h, colors="black", linestyles="dashed")
            voff=-60
            if h == 11_000_000:
                voff=-75
            ax.annotate(round(duration[i],1),
                fontsize=8,
                xy=(duration[i], 0), xycoords='data',
                xytext=(0, voff), textcoords='offset points',
                bbox=dict(boxstyle="round", fc="0.9"),
                arrowprops=dict(arrowstyle="-"),
                horizontalalignment='center', verticalalignment='bottom')


    ax.get_yaxis().get_major_formatter().set_scientific(False)
    ax.get_yaxis().get_major_formatter().set_useOffset(False)
  
    ax.margins(0)

    ax.set_axisbelow(True)

    ax.xaxis.grid(color='gray', linestyle='solid', which='major')
    ax.yaxis.grid(color='gray', linestyle='solid', which='major')

    ax.xaxis.grid(color='gray', linestyle='dashed', which='minor')
    ax.yaxis.grid(color='gray', linestyle='dashed', which='minor')
  
    xminorLocator = MultipleLocator(1.0)
    ax.xaxis.set_major_locator(xminorLocator)

    yminorLocator = MultipleLocator(500_000)
    ax.yaxis.set_major_locator(yminorLocator)


    # plt.xticks(ax.get_xticks(), ax.get_xticklabels(which='both'))
    # plt.setp(ax.get_xticklabels(which='both'), rotation=30, horizontalalignment='right')

# sort alabsle by .0.1
alabels.sort(key=lambda x: x[0][0])

lheight=40
loffset=-40
count=len(alabels)
for (duration,h), label in alabels:

    ax.annotate(label,
                fontsize=8,
                xy=(duration, h), xycoords='data',
                xytext=(loffset, lheight), textcoords='offset points',
                bbox=dict(boxstyle="round", fc="0.9"),
                arrowprops=dict(arrowstyle="->"),
                horizontalalignment='center', verticalalignment='bottom')
    loffset += round(80/count,0)
    if loffset <0:
        lheight += 20
    elif loffset > 0:
        lheight -= 20


plt.axhline(y=11_500_000, color='blue', linestyle='--')

# plt.legend(tuple(lgs), tuple(args.label), loc='upper left', shadow=True)
plt.title('CKB Block Sync progress Chart')
plt.xlabel('Timecost (hours)')
plt.ylabel('Block Height')
plt.savefig(result_path, bbox_inches='tight', dpi=300)
