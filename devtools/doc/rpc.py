#!/usr/bin/env python

import sys
import json

def newline(n):
    for i in range(0, n):
        print("")


def sort_cases_by_module(cases):
    return sorted(cases, key=lambda case: case["module"])

def print_title(case):
    print("### `{}`".format(case["method"]))
    newline(1)


def print_description(case):
    print(case["description"])
    newline(1)


def print_types(case):
    if case.get("types") is None:
        return

    print("#### Parameters")
    newline(1)
    for item in case["types"]:
        if len(item) != 1:
            raise Exception("Invalid `types` format, expect one map for only one type: {}".format(item))
        for (key, val) in item.items():
            print("    {} - {}".format(key, val))

def print_returns(case):
    if case.get("returns") is None:
        return

    print("#### Returns")
    newline(1)
    for item in case["returns"]:
        if len(item) != 1:
            raise Exception("Invalid `returns` format, expect one map for only one type: {}".format(item))
        for (key, val) in item.items():
            print("    {} - {}".format(key, val))

def print_example(case):
    example = {}
    example["id"] = case.get("id", 2)
    example["jsonrpc"] = case.get("jsonrpc", "2.0")
    example["params"] = case["params"]
    example["method"] = case["method"]

    bash = r"""
#### Examples

```bash
echo '@EXAMPLE' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```"""
    bash = bash.replace(
        "@EXAMPLE",
        json.dumps(example, sort_keys=True, indent=4, separators=(',', ': '))
    )
    print(bash)


def print_result(case):
    result = {}
    result["id"] = case.get("id", 2)
    result["jsonrpc"] = case.get("jsonrpc", "2.0")
    result["result"] = case["result"]
    bash = r"""
```json
@RESULT
```"""
    bash = bash.replace(
        "@RESULT",
        json.dumps(result, sort_keys=True, indent=4, separators=(',', ': '))
    )
    print(bash)


def print_toc(cases):
    module = ""
    for case in cases:
        if case["module"] != module:
            module = case["module"]
            print('*   [`{}`](#{})'.format(module.capitalize(), module))
        method = case["method"]
        print('    *   [`{}`](#{})'.format(method, method))
    newline(1)


def usage():
    usages = """\
Generate rpc README.md based on rpc descriptions file in json format

Usage:
    python {} /path/to/rpc.json
    """.format(__file__)
    print(usages)


def main():
    if len(sys.argv) < 2 or sys.argv[1] == "--help":
        usage()
        return

    filepath = sys.argv[1]
    with open(filepath) as f:
        cases = sort_cases_by_module(json.load(f))

    print("# CKB JSON-RPC Protocols")
    newline(1)
    print("NOTE: This file is auto-generated. Please don't update this file directly; instead make changes to `rpc/json/rpc.json` and re-run `make gen-rpc-doc`")
    newline(2)

    print_toc(cases)

    module = ""
    is_first = True
    for case in cases:
        if is_first:
            is_first = False
        else:
            newline(1)

        if case["module"] != module:
            module = case["module"]
            print('## {}'.format(module.capitalize()))
            newline(1)
        print_title(case)
        print_description(case)
        print_types(case)
        print_returns(case)
        print_example(case)
        print_result(case)


if __name__ == '__main__':
    main()
