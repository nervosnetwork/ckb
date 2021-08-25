#!/usr/bin/python3
import requests
import json
import time
import datetime
import os
import sys
from dotenv import load_dotenv
from github import Github
load_dotenv()
job_runs_info=str(os.getenv('workspace'))+"/job_runs_info.txt"
job_info=str(os.getenv('workspace'))+"/job_info.txt"
headers = {"Authorization": "token "+str(os.getenv('TOKEN'))}
def run_query(url): # A  function to use requests.get to make the API call. Note the json= section.
    request = requests.get(url,headers=headers)
    link = request.headers.get('link', None)
    if link is not None:
        print(link)
    if request.status_code == 200:
        return request.json()
    else:
        raise Exception("Query failure to run by returning code of {}. {}".format(request.status_code))
#function to call commit check suit
def get_check_suite(commit_sha):
    url="https://api.github.com/repos/"+str(os.getenv('REPOSITPRY'))+"/commits/"+commit_sha+"/check-suites"
    check_suite_result=run_query(url)
    data = {}
    data['job_run_info']=[]
    for num in range(len(check_suite_result["check_suites"])):
        if (check_suite_result["check_suites"][num]["app"]["slug"] == "github-actions"):
            data["job_run_info"].append({
           'job_run_url':check_suite_result["check_suites"][num]["check_runs_url"]
           })
    with open(job_runs_info, 'w') as outfile:
            json.dump(data, outfile)

# function to get each job info from each checkruns
def get_check_runs(commit_sha):
    get_check_suite(commit_sha)
    f = open(job_runs_info,"r")
    data = json.load(f)
    job_data={}
    job_data["job_details"]=[]
    for i in range(len(data['job_run_info'])):
        job_run_url=data['job_run_info'][i]["job_run_url"]
        job_info_res=run_query(job_run_url)
        for j in range(len(job_info_res["check_runs"])):
            print(job_info_res["check_runs"][j]["name"])
            job_data["job_details"].append({
            'job_name':job_info_res["check_runs"][j]['name'],
            'job_status':job_info_res["check_runs"][j]['status'],
            'job_conclusion':job_info_res["check_runs"][j]['conclusion'],
            'job_started_at':job_info_res["check_runs"][j]['started_at'],
            'job_completed_at':job_info_res["check_runs"][j]['completed_at']
            })
    with open(job_info, 'w') as outfile:
            json.dump(job_data, outfile)
#function to check job's conculusions
def check_runs_conculusions(commit_sha,expect_job_num,expect_jobs):
    print("check_runs_conculusions"+str(commit_sha))
    get_check_runs(commit_sha)
    f = open(job_info,"r")
    jobs_data= json.load(f)
    print("jobs_data")
    print(jobs_data)
    print("jobs_data done")
    CI_conclusion=""
    excuted_jobs_count=0
    # UnitTest conclusion
    UnitTest_macOS_conclusion=""
    UnitTest_Linux_conclusion=""
    UnitTest_Windows_conclusion=""
    UnitTest_conclusion=""
    is_UnitTest_run=False
    # Liners conclusion
    Liners_macOS_conclusion=""
    Liners_Linux_conclusion=""
    Liners_conclusion=""
    is_Liners_run=False
    #Benchmark conclusion
    Benchmark_Linux_conclusion=""
    Benchmark_macOS_conclusion=""
    Benchmark_conclusion=""
    is_Benchmark_run=False
    #Integration conclusion
    Integration_Linux_conclusion=""
    Integration_macOS_conclusion=""
    Integration_Windows_conclusion=""
    Integration_conclusion=""
    is_Integration_run=False
    #Quick check conclusion
    Quick_Check_conclusion=""
    is_Quick_Check_run=False
    #Security Audit conclusion
    Security_Audit_Licenses_conclusion=""
    is_Security_Audit_run=False
    #WASM build conclusion
    WASM_build_conclusion=""
    is_WASM_build_run=False


    for i in range(len(jobs_data["job_details"])):
        #Unit test conclusion
        job_name=jobs_data["job_details"][i]["job_name"]
        job_conclusion=jobs_data["job_details"][i]["job_conclusion"]
        for j in range(len(expect_jobs)):
           expected_name=str(expect_jobs[j])
           if (expected_name.find("unit_test")):
              is_UnitTest_run=True
              if job_name.find("ci_unit_tests") != -1 & job_name.find("ubuntu") != -1:
                  UnitTest_Linux_conclusion=job_conclusion
              if job_name.find("ci_unit_tests (macos") != -1:
                  UnitTest_macOS_conclusion=job_conclusion
              if job_name.find("ci_unit_tests (windows") != -1:
                  UnitTest_Windows_conclusion=job_conclusion
            #Integration test conclusion
           if (expected_name.find("integration_test")):
              is_Integration_run=True
              if job_name.find("ci_integration_test") != -1 & job_name.find("ubuntu") != -1:
                    Integration_Linux_conclusion=job_conclusion
              if job_name.find("ci_integration_test (macos") != -1:
                    Integration_macOS_conclusion=job_conclusion
              if job_name.find("ci_integration_test (windows") != -1:
                    Integration_conclusion=job_conclusion
            #liners test conclusion
           if (expected_name.find("liners")):
              is_Liners_run=True
              if job_name.find("ci_liners") != -1 & job_name.find("ubuntu") != -1:
                    Liners_Linux_conclusion=job_conclusion
              if job_name.find("ci_liners (macos") != -1:
                    Liners_macOS_conclusion=job_conclusion
            #Benchmark test conclusion
           if (expected_name.find("benchmarks_test")):
               is_Benchmark_run=True
               if job_name.find("ci_benchmarks_test") != -1 & job_name.find("ubuntu") != -1:
                  Benchmark_Linux_conclusion=job_conclusion
               if job_name.find("ci_benchmarks_test (macos") != -1:
                  Benchmark_macOS_conclusion=job_conclusion
            #Quick check conclusion
           if (expected_name.find("quick_check")):
               is_Quick_Check_run=False
               if job_name.find("ci_quick_check") != -1:
                  Quick_Check_conclusion=job_conclusion
            #Security Audit conclusion
           if (expected_name.find("security_audit")):
               is_Security_Audit_run=True
               if job_name.find("ci_security_audit_licenses") != -1:
                  Security_Audit_Licenses_conclusion=job_conclusion
            #WASM build conclusion
           if (expected_name.find("WASM_build")):
               is_Quick_Check_run=True
               if job_name.find("ci_WASM_build") != -1:
                  WASM_build_conclusion=job_conclusion

     #UnitTest check
    if (is_UnitTest_run == True):
        if (UnitTest_macOS_conclusion == "success" ) | (UnitTest_Linux_conclusion == "success" ) | (UnitTest_Windows_conclusion == "success" ):
            UnitTest_conclusion="success"
            excuted_jobs_count +=1
        elif (UnitTest_macOS_conclusion == "failure" ) | (UnitTest_Linux_conclusion == "failure" ) | (UnitTest_Windows_conclusion == "failure" ):
            UnitTest_conclusion="failure"
            excuted_jobs_count +=1
     #Integration check
    if (is_Integration_run == True):
        if (Integration_macOS_conclusion == "success" ) | (Integration_Linux_conclusion == "success" ) | (Integration_Windows_conclusion == "success" ):
            Integration_conclusion="success"
            excuted_jobs_count +=1
        elif (Integration_macOS_conclusion == "failure" ) | (Integration_Linux_conclusion == "failure" ) | (Integration_Windows_conclusion == "failure" ):
            Integration_conclusion="failure"
            excuted_jobs_count +=1
    # Liners check
    if (is_Liners_run == True):
        if (Liners_macOS_conclusion == "success" ) | (Liners_Linux_conclusion == "success" ):
            Liners_conclusion="success"
            excuted_jobs_count +=1
        elif (Liners_macOS_conclusion == "failure" ) | (Liners_Linux_conclusion == "failure" ):
            Liners_conclusion="failure"
            excuted_jobs_count +=1
   # Benchmark check
    if (is_Benchmark_run == True):
        if (Benchmark_Linux_conclusion == "success" ) | (Benchmark_macOS_conclusion == "success" ):
            Benchmark_conclusion="success"
            excuted_jobs_count +=1
        elif (Benchmark_Linux_conclusion == "failure" ) | (Benchmark_macOS_conclusion == "failure" ):
            Benchmark_conclusion="failure"
            excuted_jobs_count +=1 
    # Quick_Check check
    if (is_Quick_Check_run == True):
        excuted_jobs_count +=1
    # Security_Audit check
    if (is_Security_Audit_run == True):
        excuted_jobs_count +=1
    # WASM_build check
    if (is_WASM_build_run == True):
        excuted_jobs_count +=1

    jobs_conclusion=[UnitTest_conclusion,Liners_conclusion,Benchmark_conclusion,Integration_conclusion,Quick_Check_conclusion,Security_Audit_Licenses_conclusion,WASM_build_conclusion]
    # check child jobs conclusions if all required jobs completed in one os
    print("excuted_jobs_count is "+str(excuted_jobs_count))
    print("expect_job_num is "+str(expect_job_num))
    if ( excuted_jobs_count == expect_job_num ):
        #set ci conclusion
        if  "failure" in jobs_conclusion:
            CI_conclusion="failure"
        else:
            CI_conclusion="success"
    #create required job ci
    if ( (os.getenv('EVENT_NAME') == "pull_request") | (os.getenv('ACTOR') == "bors[bot]") ) & ( CI_conclusion == "success" ):
       update_commit_state(COMMIT_SHA)

#function to create reqiured job ci
def update_commit_state(COMMIT_SHA):
    g = Github(os.getenv('TOKEN'))
    repo = g.get_repo(os.getenv('REPOSITPRY'))
    repo.get_commit(sha=COMMIT_SHA).create_status(
        state="success",
        description="ci",
        context="ci"
    )
if __name__ == '__main__':

   COMMIT_SHA=''
   MESSAGE=''
   REPO_LIST=["janx", "doitian", "quake", "xxuejie", "zhangsoledad", "jjyr", "TheWaWaR", "driftluo", "keroro520", "yangby-cryptape","liya2017"]
   if str(os.getenv('EVENT_NAME')) == "push":
      COMMIT_SHA=str(os.getenv('COMMIT_SHA'))
      MESSAGE=str(os.getenv('COMMIT_MESSAGE'))

   if str(os.getenv('EVENT_NAME')) == "pull_request":
      COMMIT_SHA=str(os.getenv('PR_COMMIT_SHA'))
      MESSAGE=str(os.getenv('PR_COMMONS_BODY'))

   if ( "ci:" in MESSAGE ) & ( ( os.getenv('EVENT_NAME') == "push" ) | ( ( os.getenv('REPO_OWNER') == "nervosnetwork" ) & ( os.getenv('EVENT_NAME') == "pull_request" ) & ( os.getenv('REPO_ACTOR') in REPO_LIST ) ) ):
        print("spicific ci jobs run")
        required_job=MESSAGE.split("ci:[")[1].split("]")[0].split(',')
        update_commit_state(COMMIT_SHA)
        check_runs_conculusions(COMMIT_SHA,len(required_job),required_job)
   else :
       required_job=['ci_unit_tests','ci_integration_test','ci_liners','ci_benchmarks_test','ci_quick_check','ci_security_audit_licenses','ci_WASM_build']
       check_runs_conculusions(COMMIT_SHA,7,required_job)
