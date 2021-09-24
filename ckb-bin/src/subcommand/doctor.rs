use std::path::Path;

use bugreport::{bugreport, collector::*, format::Markdown};
use sysinfo::{NetworkExt, System, SystemExt};

use ckb_app_config::{DoctorArgs, ExitCode};
use ckb_chain_spec::ChainSpec;

struct Environment {
    ckb_version: String,
    os_info: String,
    compile_info: String,
    disk_info: String,
    cpu_info: String,
    mem_info: String,
    kernel_info: String,
    cfg_content: Vec<String>,
}

/// strip "abc" from "abc\n\n1234", return "1234"
fn strip_title(content: &str) -> &str {
    //if match, pass '\n\n'; or head of content
    let pass_title = content.find("\n\n").map_or(0, |i| i + 2);
    content.split_at(pass_title).1.trim()
}

/// make content(list in markdown) into level-2 list
fn make_level2_list(content: &str) -> String {
    let mut new_string = content.replacen('\n', "\n  ", 9);
    new_string.insert_str(0, "  ");
    new_string
}

impl Environment {
    fn create() -> Self {
        let ckb_version = String::from(strip_title(
            &bugreport!()
                .info(SoftwareVersion::default())
                .format::<Markdown>(),
        ));
        let compile_info = make_level2_list(strip_title(
            &bugreport!()
                .info(CompileTimeInformation::default())
                .format::<Markdown>(),
        ));

        let mut sys = System::new_all();
        sys.refresh_all();

        let os_info: String = [
            sys.name().unwrap_or_else(|| String::from("UNKNOWN")),
            sys.os_version().unwrap_or_else(|| String::from("UNKNOWN")),
        ]
        .join(" ");

        let mut disk_info = String::new();
        for disk in sys.disks() {
            disk_info.push_str(&format!("{:?}\n", disk));
        }

        let mut network_info = String::new();
        for (interface_name, data) in sys.networks() {
            network_info.push_str(&format!(
                "{}: {}/{} B\n",
                interface_name,
                data.received(),
                data.transmitted()
            ));
        }

        let mut cpu_info = String::new();
        for component in sys.components() {
            cpu_info.push_str(&format!("{:?}\n", component));
        }

        let mut mem_info = String::new();
        mem_info.push_str(&format!("total memory: {} KB\n", sys.total_memory()));
        mem_info.push_str(&format!("used memory : {} KB\n", sys.used_memory()));
        mem_info.push_str(&format!("total swap  : {} KB\n", sys.total_swap()));
        mem_info.push_str(&format!("used swap   : {} KB\n", sys.used_swap()));

        let cfg_content = vec![String::from(strip_title(
            &bugreport!()
                .info(FileContent::new("ckb.toml", Path::new("./ckb.toml")))
                .format::<Markdown>(),
        ))];

        let kernel_info = sys
            .kernel_version()
            .unwrap_or_else(|| String::from("UNKNOWN"));

        Self {
            os_info,
            ckb_version,
            compile_info,
            disk_info,
            cpu_info,
            kernel_info,
            mem_info,
            cfg_content,
        }
    }
}

/// transfer to normal chain name
fn chain_name(chain: String) -> &'static str {
    match chain.as_str() {
        "ckb" => "mainnet",
        "ckb_dev" => "dev",
        "ckb_testnet" => "testnet",
        "ckb_staging" => "staging",
        _ => "unknown",
    }
}

fn make_github_issue(chain: ChainSpec, environment: Environment) -> String {
    let body = format!(
"
## Bug Report

<details><summary>Bug Report Guideline</summary>

### What is a Useful Bug Report

Useful bug reports are ones that get bugs fixed. A useful bug report is...

1. Reproducible - If an engineer can't see it or conclusively prove that it exists, the engineer will probably stamp it WORKSFORME or INVALID, and move on to the next bug.
2. Specific - The quicker the engineer can trace down the issue to a specific problem, the more likely it'll be fixed expediently.

So the goals of a bug report are to:

- Pinpoint the bug
- Explain it to the developer

Your job is to figure out exactly what the problem is.

### Bug Reporting General Guidelines

- Avoid duplicates: Search before you file!
- Always test the latest available build.
- One bug per report.
- State useful facts, not opinions or complaints.
- Flag security/privacy vulnerabilities as non-public.

</details>

### Status Update

- **Severity**: P1 / P2 / P3
- **Priority**: Now / Later
- **Happened at**: <!-- when the bug happened -->
- **Assigned to**:
- **Reported by**:
- **Status**: Investigating / Root Cause Located

### Current Behavior
<!-- A clear and concise description of the behavior. -->

### Expected Behavior
<!-- A clear and concise description of what you expected to happen. -->

### Steps to reproduce
<!-- How you encounter the bug? Can you reproduce it now and summarize the steps? -->

### Possible Solution
<!-- Only if you have suggestions on a fix for the bug -->

### Reduced Test Case
<!-- The goal of test case is to pinpoint the bug. A Reduced Test Case rips out everything in the page that is not required to reproduce the bug. Also try variations on the test case to find related situations that also trigger the bug. -->

### Environment Setup and Configuration
<!--
Please also include the environment setup and configuration information, such as OS, system build and platform etc.
-->

- **CKB version**: {}
- **Chain**: {}
- **Installation**: [GitHub Release, Built from source]
- **Operating system**: {}
- **CompileInfo**:
{}
- **Kernel**: {}

<details><summary>More System Info</summary>

- cpus:
{}
- memory:
{}
- disks:
{}

</details>

#### Existing Environment for Debug

- How to login.
- How to locate related information.
- What need to do to restore the environment after investigation

### Additional context/Screenshots
<!-- Add any other context about the problem here. If applicable, add screenshots to help explain. -->

#### Config files

<details><summary>ckb.toml</summary>

{}

</details>

<!-- Add ckb-miner.toml if the bug is related to the miner -->",
        environment.ckb_version,
        chain_name(chain.name),
        environment.os_info,
        &environment.compile_info,
        &environment.kernel_info ,
        &environment.cpu_info,
        &environment.mem_info,
        &environment.disk_info,
        &environment.cfg_content.get(0).unwrap(),
    );

    body
}

pub fn doctor(args: DoctorArgs) -> Result<(), ExitCode> {
    let chain = args.chain;
    if args.gen_report.is_some() {
        // generate report
        let mut report_file = std::env::current_dir()?;
        report_file.push("bug_report.md");

        let environment = Environment::create();
        let bug_content = make_github_issue(chain, environment);
        std::fs::write(&report_file, bug_content)?;
        println!("...Save BugReport to {:?}", &report_file);
        println!(
            "If user want to submit a new issue, please paste below link to web browser, and fill in BugReport content\
        \n\nhttps://github.com/nervosnetwork/ckb/issues/new?template=Bug_report.md&labels=t%3Abug\n"
        );
    }

    Ok(())
}
