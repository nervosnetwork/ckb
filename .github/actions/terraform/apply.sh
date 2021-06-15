#!/bin/bash

# Call by action `entrypoint`

SCRIPT_PATH="$( cd -- "$(dirname "$0")" >/dev/null 2>&1 ; pwd -P )"
${SCRIPT_PATH}/main.sh apply
