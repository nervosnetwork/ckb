#!/bin/bash
set -e
fun_run_os(){
  runs_on="$1"
  github_workflow_os=`echo $GITHUB_WORKFLOW | awk -F '_' '{print $NF}'`
  echo "github_workflow_os is "$github_workflow_os
  os_skip="skip"
  echo "$runs_on" | sed "s/\[//g" | sed "s/\]//g" | sed "s/,/\n/g" > runs_on.txt
  while read -r LINE;
  do
    LINE="$(echo "$LINE" | tr -d '[:space:]')"
    echo "OS LINE is "$LINE
      if [[ $github_workflow_os == $LINE  ]];then
        os_skip="run"
      fi
  done < "runs_on.txt"
  echo "OS skip is "$os_skip
  echo "::set-output name=os_skip::$os_skip"
}

fun_jobs(){
  job_list="$1"
  job_skip="skip"
  echo "$job_list" | sed "s/\[//g" | sed "s/\]//g" | sed "s/,/\n/g" > job_run.txt
  while read -r LINE;
  do
    LINE="ci_$(echo "$LINE" | tr -d '[:space:]')"
    if [[ $GITHUB_WORKFLOW == "$LINE"* ]];then
      echo "job_name is"$LINE
      echo $GITHUB_WORKFLOW
      job_skip="run"
    fi
  done < "job_run.txt"
  echo "JOB skip is" $job_skip
  echo "::set-output name=job_skip::$job_skip"
}

fun_pasing_message(){
  set +e
  MESSAGE=$1
  #pass job run list
  echo "$MESSAGE" | grep -q "ci-runs-only:"
  if [ $? -eq 0 ]; then
    job_run_list=`echo "${MESSAGE}"| grep "ci-runs-only" | awk -F ':' '{print $2}'`
  else
    job_run_list=" [ quick_checks,unit_tests,integration_tests,benchmarks,linters,wasm_build,cargo_deny ] "
  fi
  echo "job_run_list is ""$job_run_list"
  #parsing runs os
  echo "$MESSAGE" | grep -q "ci-runs-on:"
  if [ $? -eq 0 ]; then
     runs_on=` echo "${MESSAGE}"| grep "ci-runs-on" | awk -F ':' '{print $2}'`
  else
     runs_on=" [ ubuntu,macos,windows ] "
  fi
  echo "runs_on is ""$runs_on"

  #parsing uses-self-runner
  echo "$MESSAGE" | grep -q "ci-uses-self-runner:"
  if [ $? -eq 0 ]; then
     ci_uses_self_runner=` echo "${MESSAGE}"| grep "ci-uses-self-runner" | awk -F ':' '{print $2}' | awk '{gsub(/^\s+|\s+$/, "");print}'`
  else
     ci_uses_self_runner=false
  fi
  echo "ci_uses_self_runner is ""$ci_uses_self_runner"
  set -e
  #set reqiured output
  fun_run_os "$runs_on"
  fun_jobs "$job_run_list"
  echo "GITHUB_REPOSITORY is "$GITHUB_REPOSITORY
  if [[ "$ci_uses_self_runner" == "true" ]] || [[ "$GITHUB_REPOSITORY" == "nervosnetwork/ckb" ]];then
    linux_runner_label='self-hosted-ci-ubuntu-20.04'
    windows_runner_label='self-hosted-ci-windows-2019'
  else
    linux_runner_label='ubuntu-20.04'
    windows_runner_label='windows-2019'
  fi
  echo "linux_runner_label is "$linux_runner_label
  echo "windows_runner_label is "$windows_runner_label
  echo "::set-output name=linux_runner_label::$linux_runner_label"
  echo "::set-output name=windows_runner_label::$windows_runner_label"
}

if [[ $GITHUB_EVENT_NAME == "push" ]];then
  MESSAGE="$COMMIT_MESSAGE"
  fun_pasing_message "$MESSAGE"
fi
if [[ $GITHUB_EVENT_NAME == "pull_request" ]];then
  MESSAGE="$PR_COMMONS_BODY"
  actor_permission=`curl -H "Authorization: Bearer $GITHUB_TOKEN" https://api.github.com/repos/$GITHUB_REPOSITORY/collaborators/$GITHUB_ACTOR/permission |jq .permission | sed 's/\"//g'`
  echo "actor_permission is" $actor_permission
  if [[ ${LABELS[@]} =~ "ci-trust" ]];then
    echo "ci-trust"
    fun_pasing_message "$MESSAGE"
  elif [[ $actor_permission == "admin" || $actor_permission == "write" ]];then
    echo "actor_permission"
    fun_pasing_message "$MESSAGE"
  else
    runs_on=" [ ubuntu,macos,windows ] "
    fun_run_os "$runs_on"
    job_run_list=" [ quick_checks,unit_tests,integration_tests,benchmarks,linters,wasm_build,cargo_deny ] "
    fun_jobs "$job_run_list"
    if [[ "$GITHUB_REPOSITORY" == "nervosnetwork/ckb" ]];then
      echo "::set-output name=linux_runner_label::self-hosted-ci-ubuntu-20.04"
      echo "::set-output name=windows_runner_label::self-hosted-ci-windows-2019"
    else
      echo "::set-output name=linux_runner_label::ubuntu-20.04"
      echo "::set-output name=windows_runner_label::windows-2019"
    fi
  fi
fi
