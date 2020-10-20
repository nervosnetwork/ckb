#!/usr/bin/env python3

import re
import sys
import copy
from pathlib import Path
from collections import defaultdict

crate_deps = dict()
crate_deps_reverse = defaultdict(set)

CHECK_DEV_DEPS = sys.argv[1] == '--dev' if len(sys.argv) > 1 else False
LOCAL_DEP_RE = re.compile(r'{.*path\s*=\s*"([^"]*)"')
TOP_DIR = Path('.').resolve()

members = []
with Path('Cargo.toml').open() as top_cargo:
    parsing_members = False
    for line in top_cargo.readlines():
        if parsing_members:
            if line.strip() == ']':
                break
            if line.strip().startswith('"'):
                members.append(Path(line.split('"')[1]))
            elif line.strip().startswith("'"):
                members.append(Path(line.split("'")[1]))
        elif [w.strip() for w in line.split('=')] == ['members', '[']:
            parsing_members = True

if len(members) == 0:
    print("Failed to parse members from ./Cargo.toml", file=sys.stderr)
    sys.exit(127)

for crate_dir in members:
    crate_deps[crate_dir] = set()
    with (crate_dir / 'Cargo.toml').open() as f:
        for line in f.readlines():
            if line.strip() == '[dev-dependencies]'.strip() and not CHECK_DEV_DEPS:
                break
            match = LOCAL_DEP_RE.search(line)
            if match is not None:
                dep_dir = (crate_dir / match.group(1)).resolve().relative_to(TOP_DIR)
                crate_deps[crate_dir].add(dep_dir)
                crate_deps_reverse[dep_dir].add(crate_dir)

has_missing_members = False
for dep in set(dep for deps in crate_deps.values() for dep in deps):
    if dep not in crate_deps:
        has_missing_members = True
        print("Member {} is missing in ./Cargo.toml".format(dep), file=sys.stderr)
if has_missing_members:
    sys.exit(127)

remember_crate_deps = copy.deepcopy(crate_deps)
while len(crate_deps) > 0:
    available = [crate for (crate, deps) in crate_deps.items() if len(deps) == 0]
    if len(available) == 0:
        print("Loop dependencies detected:\n", file=sys.stderr)
        for (k, v) in sorted(crate_deps.items(), key=lambda t: len(t[1])):
            print("|- ", k, file=sys.stderr)
            for e in v:
                print("|   |- ", e, file=sys.stderr)
        sys.exit(127)

    for crate in available:
        print(crate)
        for crate_user in crate_deps_reverse[crate]:
            crate_deps[crate_user].remove(crate)
        del crate_deps[crate]

crate_deps = remember_crate_deps
for crate in members:
    if len(crate_deps[crate]) > 0:
        print("Workspace members are not sorted by dependencies, see the topological sorted printed above", file=sys.stderr)
        print("{} depends on following members which come after it:".format(crate), file=sys.stderr)
        for e in crate_deps[crate]:
            print("    |- ", e, file=sys.stderr)
    for crate_user in crate_deps_reverse[crate]:
        crate_deps[crate_user].remove(crate)
