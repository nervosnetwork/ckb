#!/usr/bin/env python

from __future__ import print_function

import json
import sys
from collections import OrderedDict

for file in sys.argv[1:]:
    with open(file) as fp:
        loaded = json.load(fp, object_pairs_hook=OrderedDict)

    with open(file, 'w') as fp:
        for line in json.dumps(loaded, indent=4).splitlines():
            print(line.rstrip(), file=fp)
