#!/usr/bin/env bash

# Usage Example
#
# ```shell
# ./render_main_tf.sh ap-northeast-1 ap-northeast-2 > main.tf
# ```

regions="$@"

for region in $regions; do
    sed "s/ap-northeast-1/$region/g" main.tf.template
done
