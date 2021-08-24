#!/bin/bash
set -e
set -u
while [[ $# -gt 0 ]]
do
[ -n "${DEBUG:-}" ] && set -x || true

set +e
"$@"
EXIT_STATUS="$?"
set -e
if [ "$EXIT_STATUS" = 0 ]; then
    echo "Check whether the ci succeeds"
else
    echo "Fail the ci"
fi
exit $EXIT_STATUS