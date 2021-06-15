#!/usr/bin/env bash

# # Intention
#
# We plan to apply multiple ec2 instances on different regions. While I didn't
# find a simply way to do this on Terraform. As a result, I created this script.
# It accepts a list of regions and generates Terraform configurations that
# declares multiple ec2 instances on these given regions.
#
# # Usage Example
#
# ```shell
# ./render_main_tf.sh ap-northeast-1 ap-northeast-2 > main.tf
# ```

SCRIPT_PATH="$( cd -- "$(dirname "$0")" >/dev/null 2>&1 ; pwd -P )"
TEMPLATE_PATH=$SCRIPT_PATH/main.tf.template
TEMPLATE_REGION="ap-northeast-1"
regions="$@"

# Check duplicates
duplicated=$(echo "$regions" \
        | tr ' ' '\n' \
        | awk '
            BEGIN { duplicated = "" }
            {
                count[$1]++;
                if (count[$1] > 1) duplicated=$1
            }
            END { print duplicated }
        ')
if [ ! -z "$duplicated" ]; then
    echo "ERROR: Duplicated regions \"${duplicated}\""
fi

for region in $regions; do
    sed "s/$TEMPLATE_REGION/$region/g" $TEMPLATE_PATH
done
