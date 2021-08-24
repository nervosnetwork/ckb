#!/bin/bash
set +e
nervosnetwork_actor_list='"janx", "doitian", "quake", "xxuejie", "zhangsoledad", "jjyr", "TheWaWaR", "driftluo", "keroro520", "yangby-cryptape"'
actor="${ACTOR}"
# echo "${COMMIT_MESSAGE}" | grep "runs_on"
echo $COMMIT_MESSAGE | grep -q "runs_on:"
if [ $? -eq 0 ]; then 
    runs_on=` echo "${COMMIT_MESSAGE}"| grep "runs_on" | awk -F 'runs-on:' '{print $1}' | awk -F ':' '{print $2}'`
elif [ ! -n ${CI_RUNS_ON} ]; then
    runs_on='${{secrets.CI_RUNS_ON}}'
elif [[ ${REPO_OWNER} != "nervosnetwork" ]] || [[ ${REPO_OWNER} == "nervosnetwork" && ${EVENT_NAME}} == 'pull_request' && $nervosnetwork_actor_list != *$REPO_ACTOR* ]]; then 
    runs_on=' [ "ubuntu-18.04","macos-10.15","windows-2019" ] '
else
    runs_on=' [ "self-hosted-ci-ubuntu-20.04",macos-10.15","windows-2019" ] '
fi
echo  $runs_on
# runs_on='["ubuntu-18.04","macos-10.15"]'
# echo "::set-output name=matrix::{\"os\":$runs_on}"

