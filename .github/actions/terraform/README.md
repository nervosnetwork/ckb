# Terraform

Suppose you have a job that applies AWS resources, runs your tests on these resources, and finally, destroys them whatever succeeded, failed or aborted. You can do this safely by writing Terraform configuration files and passing the directory path.

This action use [`post`](https://github.com/actions/runner/blob/be9632302ceef50bfb36ea998cea9c94c75e5d4d/docs/adrs/0361-wrapper-action.md) keyword to ensure that clean work always be done.

# Usage

<!-- start usage -->
```yaml
- uses: ./.github/actions/terraform
  with:
    # Required, directory path to Terraform configuration files
    terraform_dir: /path/to/terraform/configuration/directory
```
<!-- end usage -->

# Examples

```yaml
# Generate a random SSH key pair to access instances, such as Ansible, via SSH
- run: ssh-keygen -N "" -f ${{ env.PRIVATE_KEY_PATH }}

- name: Apply Resources
  uses: ./.github/actions/terraform
  env:
    # You may declare Terraform variables in `variables.tf`.
    #
    # Setting declared variables as environment variables is a common practice.
    #
    # [Learn more about Terraform variables](https://www.terraform.io/docs/language/values/variables.html#environment-variables)
    TF_VAR_access_key:       ${{ secrets.AWS_ACCESS_KEY }}
    TF_VAR_secret_key:       ${{ secrets.AWS_SECRET_KEY }}
    TF_VAR_private_key_path: ${{ env.PRIVATE_KEY_PATH }}
    TF_VAR_public_key_path:  ${{ env.PUBLIC_KEY_PATH }}
    TF_VAR_instance_type:    t3.micro
  with:
    terraform_dir: /path/to/terraform/configuration/directory

- name: Output
  working-directory: /path/to/terraform/configuration/directory
  run: |
    terraform output -raw "<ansible inventory defined as Terraform output>" >> /path/to/ansible/inventory-file

# Ansible-playbook do tests
- run: ansible-playbook playbook.yml -i /path/to/ansible/inventory-file ...
```
