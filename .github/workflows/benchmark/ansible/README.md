# Ansible Playbook Used For Benchmark Workflow

This Ansible playbook is part of the "Benchmark" workflow.

## Pre-Install

```
ansible-galaxy install -r requirements.yml
```

## Inventory File

To use this playbook, the provided inventory file must have 2 groups of hosts:
  * Group "instances" indicates CKB nodes and hosts named by "instance-0", "instance-1" and so on.

  * Group "bastions" indicates CKB-Benchmark nodes, only have one host, named "bastion-0"

All hosts must have `instance_type` variables, which are used to generate the benchmark report.

Here is an example:

```yaml
instances:
  hosts:
    instance-0:
      ansible_user: ubuntu
      ansible_host: 1.23.45.123
      instance_type: c5.large
    instance-0:
      ansible_user: ubuntu
      ansible_host: 1.23.45.124
      instance_type: c5.large

bastions:
  hosts:
    bastion-0:
      ansible_user: ubuntu
      ansible_host: 1.23.45.125
      instance_type: t2.large
```

## Usage Example

```bash
export ANSIBLE_INVENTORY=<path to your inventory file>
export ANSIBLE_PRIVATE_KEY_FILE=<path to your privary key file>

# Install CKB on group instances
ansible-playbook playbook.yml -e 'hostname=instances' -t ckb_install,ckb_configure

# Install CKB-Benchmark on hosts bastion-0
ansible-playbook playbook.yml -e 'hostname=bastions'  -t ckb_benchmark_install,ckb_benchmark_configure

# Connect all CKB nodes into a network.
#
# In order to resolve network issues caused by IBD, we allowed instance-0 out
# of IBD, then restarted the other nodes to allow them to connect.
ansible-playbook playbook.yml -e 'hostname=instances'  -t ckb_stop
ansible-playbook playbook.yml -e 'hostname=instance-0' -t ckb_start
ansible-playbook playbook.yml -e 'hostname=instance-0' -t ckb_miner_start
sleep 5
ansible-playbook playbook.yml -e 'hostname=instance-0' -t ckb_miner_stop
ansible-playbook playbook.yml -e 'hostname=instances'  -t ckb_start

# Start benchmark
ansible-playbook playbook.yml -e 'hostname=bastions'   -t ckb_benchmark_start

# Fetch and process result
# It will produce `report.yml`, `metrics.yml` and `result.tar.xz`
ansible-playbook playbook.yml -e 'hostname=bastions'   -t fetch_ckb_benchmark_logfiles
ansible-playbook playbook.yml -e 'hostname=instances'  -t fetch_ckb_logfiles
ansible-playbook playbook.yml -e 'hostname=instances'  -t process_result
```
