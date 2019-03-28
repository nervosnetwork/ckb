#!/usr/bin/env python

import re
import sys
import subprocess
from collections import namedtuple, OrderedDict

if len(sys.argv) > 1:
    since = sys.argv[1]
else:
    tag_rev = subprocess.check_output(
        ['git', 'rev-list', '--tags', '--max-count=1']).strip()
    since = subprocess.check_output(
        ['git', 'describe', '--tags', tag_rev]).strip()

logs = subprocess.check_output(
    ['git', 'log', '--merges', '{}...HEAD'.format(since)])

START_RE = re.compile(r'\s+(\d+): (?:(\w+)(\([^\)]+\))?: )?(.*r=.*)')
END_RE = re.compile(r'\s+Co-authored-by:')

Change = namedtuple('Change', ['scope', 'module', 'title', 'text'])

current_change = None
changes = OrderedDict()
for scope in ['feature', 'fix']:
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

for line in logs.splitlines():
    if current_change:
        if not END_RE.match(line):
            current_change.text.append(line)
        else:
            if current_change.scope not in changes:
                changes[current_change.scope] = []
            changes[current_change.scope].append(current_change)
            current_change = None

    else:
        start_match = START_RE.match(line)
        if start_match:
            id, scope, module, message = start_match.groups()
            scope = SCOPE_MAPPING.get(scope, scope)
            if not scope:
                continue

            if module:
                title = '* #{0} **#{1}**: {2}'.format(id, module, message)
            else:
                title = '* #{0}: {1}'.format(id, message)

            current_change = Change(scope, module, title, [])

for scope, changes in changes.items():
    if len(changes) == 0:
        continue

    scope_title = SCOPE_TITLE.get(scope, scope.title())
    print('### {}'.format(scope_title))
    print('')

    for change in changes:
        print(change.title)
        last_is_empty = False
        for line in change.text:
            if line.strip() != '':
                print(line)
                last_is_empty = False
            else:
                if not last_is_empty:
                    print('')
                last_is_empty = True

    print('')
