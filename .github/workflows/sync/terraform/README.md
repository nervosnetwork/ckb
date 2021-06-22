# Terraform Configuration Files Used For "Sync-Mainnet" Workflow

These Terraform configuration files are part of the ["Sync-Mainnet" workflow](../../sync.yml).

## AMI

Read [`ami.tf`](./ami.tf)

## Variables

Read [`variables.tf`](./variables.tf)

## Resources

All resources are named with prefix [`var.prefix`](./variables.tf#L21)

## Outputs

* [`ansible-hosts*`](./main.tf)

## Usage Example

* Apply resources

  ```bash
  export TF_VAR_access_key=<AWS access key>
  export TF_VAR_secret_key=<AWS secret key>
  export TF_VAR_prefix="this-is-an-example"
  export TF_VAR_public_key_path=<path to ssh public key>
  export TF_VAR_private_key_path=<path to ssh private key>
  terraform init
  terraform plan
  terraform apply -auto-approve
  ```

* Destroy resources

  ```bash
  export TF_VAR_access_key=<AWS access key>
  export TF_VAR_secret_key=<AWS secret key>
  export TF_VAR_prefix="this-is-an-example"
  export TF_VAR_public_key_path=<path to ssh public key>
  export TF_VAR_private_key_path=<path to ssh private key>
  terraform destroy -auto-approve
  ```
