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


os.makedirs("changes", exist_ok=True)

if len(sys.argv) > 1:
    since = sys.argv[1]
else:
    tag_rev = _str(subprocess.check_output(
        ['git', 'rev-list', '--tags', '--max-count=1']).strip())
    since = _str(subprocess.check_output(
        ['git', 'describe', '--tags', tag_rev]).strip())

logs = _str(subprocess.check_output(
    ['git', 'log', '--merges', '--first-parent', '--pretty=tformat:%s', '{}...HEAD'.format(since)]))

PR_NUMBER_RE = re.compile(r'\s*Merge pull request #(\d+) from')
PR_TITLE_RE = re.compile(r'(?:\[[^]+]\]\s*)*(?:(\w+)(\([^\)]+\))?: )?(.*)')

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
        cache_file = "changes/{}.json".format(pr_number)
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
        if module:
            title = '* #{0} **{1}**: {2} (@{3})'.format(pr_number,
                                                        module, message, user)
        else:
            title = '* #{0}: {1} (@{2})'.format(pr_number, message, user)

        change = Change(scope, module, title, [])
        Change = namedtuple('Change', ['scope', 'module', 'title', 'text'])

        if scope not in changes:
            changes[scope] = []
        changes[scope].append(Change(scope, module, title, pr['body'] or ""))

out = open("changes/out.md", "w")
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
