# Terraform Configuration Files Used For Benchmark Workflow

These Terraform configuration files are part of the ["Benchmark" workflow](../../benchmark.yml).

## AMI

Read [`ami.tf`](./ami.tf)

## Variables

Read [`variables.tf`](./variables.tf)

## Resources

All resources are named with prefix [`var.prefix`](./variables.tf#L33)

* 1 bastion node named `"bastion-0"`
* [`var.instances_count`](./variables.tf#L17) instances nodes named `"instance-*"`
* `aws_vpc`
* `aws_key_pair`

## Outputs

* [`ansible_hosts`](./main.tf#L161)
