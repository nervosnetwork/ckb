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

ansible-playbook playbook.yml -e 'hostname=instances' -t ckb_install,ckb_configure
ansible-playbook playbook.yml -e 'hostname=instances' -t ckb_start

ansible-playbook playbook.yml -e 'hostname=bastions'  -t ckb_benchmark_install
ansible-playbook playbook.yml -e 'hostname=bastions'  -t ckb_benchmark_prepare
ansible-playbook playbook.yml -e 'hostname=bastions'  -t ckb_benchmark_start
ansible-playbook playbook.yml -e 'hostname=bastions'  -t process_result
```
