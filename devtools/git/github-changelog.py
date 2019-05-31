#!/usr/bin/env python

import os
import re
import sys
import subprocess
import json
from collections import namedtuple, OrderedDict
import requests
from requests.auth import HTTPBasicAuth


def _str(s):
    if sys.version_info >= (3, 0):
        return s.decode('utf-8')
    return s


os.makedirs(".git/changes", exist_ok=True)

if len(sys.argv) > 1:
    since = sys.argv[1]
else:
    tag_rev = _str(subprocess.check_output(
        ['git', 'rev-list', '--tags', '--max-count=1']).strip())
    since = _str(subprocess.check_output(
        ['git', 'describe', '--tags', tag_rev]).strip())

logs = _str(subprocess.check_output(
    ['git', 'log', '--reverse', '--merges', '--first-parent', '--pretty=tformat:%s', '{}...HEAD'.format(since)]))

PR_NUMBER_RE = re.compile(r'\s*Merge pull request #(\d+) from')
PR_TITLE_RE = re.compile(
    r'(?:\[[^]]+\]\s*)*(?:(\w+)(?:\(([^\)]+)\))?:\s*)?(.*)')

Change = namedtuple('Change', ['scope', 'module', 'title', 'text'])

changes = OrderedDict()
for scope in ['feat', 'fix']:
    changes[scope] = []

SCOPE_MAPPING = {
    'bug': 'fix',
    'bugs': 'fix',
    'chore': False,
    'docs': False,
    'feature': 'feat',
    'perf': 'refactor',
    'test': False,
}

SCOPE_TITLE = {
    'feat': 'Features',
    'fix': 'Bug Fixes',
    'refactor': 'Improvements',
}

auth = HTTPBasicAuth('', os.environ['GITHUB_ACCESS_TOKEN'])

for line in logs.splitlines():
    pr_number_match = PR_NUMBER_RE.match(line)

    if pr_number_match:
        pr_number = pr_number_match.group(1)
        cache_file = ".git/changes/{}.json".format(pr_number)
        if os.path.exists(cache_file):
            print("read pr #" + pr_number, file=sys.stderr)
            with open(cache_file) as fd:
                pr = json.load(fd)
        else:
            print("get pr #" + pr_number, file=sys.stderr)
            pr = requests.get('https://api.github.com/repos/nervosnetwork/ckb/pulls/' +
                              pr_number, auth=auth).json()

            if 'message' in pr:
                print(pr['message'], file=sys.stderr)
                sys.exit(1)

            with open(cache_file, 'w') as fd:
                json.dump(pr, fd)

        scope, module, message = PR_TITLE_RE.match(pr['title']).groups()
        if not scope:
            scope = 'misc'
        scope = SCOPE_MAPPING.get(scope, scope)
        if not scope:
            continue

        user = pr['user']['login']
        message = message.strip()
        message = message[0].upper() + message[1:]
        if module:
            title = '* #{0} **{1}:** {2} (@{3})'.format(pr_number,
                                                        module, message, user)
        else:
            title = '* #{0}: {1} (@{2})'.format(pr_number, message, user)

        change = Change(scope, module, title, [])
        Change = namedtuple('Change', ['scope', 'module', 'title', 'text'])

        if scope not in changes:
            changes[scope] = []

        body = pr['body'] or ""
        labels = [label['name'] for label in pr['labels']]
        is_breaking = "breaking change" in labels or any(
            l.startswith('b:') for l in labels)
        if is_breaking:
            breaking_banner = ", ".join(
                l for l in labels if l.startswith('b:'))
            if breaking_banner != "" or "breaking change" not in body.lower():
                if breaking_banner == "":
                    breaking_banner = "This is a breaking change"
                else:
                    breaking_banner = "This is a breaking change: " + breaking_banner
            if body == "":
                body = breaking_banner
            elif breaking_banner != "":
                body = breaking_banner + "\n\n" + body

        changes[scope].append(Change(scope, module, title, body))

if os.path.exists(".git/changes/extra.json"):
    with open(".git/changes/extra.json") as fin:
        extra = json.load(fin)
    for (scope, extra_changes) in extra.items():
        if scope not in changes:
            changes[scope] = []

        for change in extra_changes:
            changes[scope].append(
                Change(scope, change.get('module'), change['title'], change.get('text', '')))

out = open(".git/changes/out.md", "w")
for scope, changes in changes.items():
    if len(changes) == 0:
        continue

    scope_title = SCOPE_TITLE.get(scope, scope.title())
    print('### {}'.format(scope_title), file=out)
    print('', file=out)

    for change in changes:
        print(change.title, file=out)
        if change.text != '':
            print('', file=out)
            for line in change.text.splitlines():
                print('    ' + line, file=out)
            print('', file=out)

    print('', file=out)
