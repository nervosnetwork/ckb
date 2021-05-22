# Ansible Playbook Used For Sync-Mainnet Workflow

This Ansible playbook is part of the "Sync-Mainnet" workflow.

## Pre-Install

```
ansible-galaxy install -r requirements.yml
```

## Usage Example

```bash
export ANSIBLE_INVENTORY=<path to your inventory file>
export ANSIBLE_PRIVATE_KEY_FILE=<path to your privary key file>

ansible-playbook playbook.yml -t ckb_install,ckb_configure
ansible-playbook playbook.yml -t wait_ckb_synchronization
ansible-playbook playbook.yml -t fetch_ckb_logfiles

# Will produce report.yml within ANSIBLE_DIR
ansible-playbook playbook.yml -t process_result
```

## Variables

* `ckb_sync_target_number`, default unset, specify the synchronization target block number. If unset, the synchronization goal is the current mainnet tip.
