variable "access_key" {
  type = string
  description = "AWS access key"
}

variable "secret_key" {
  type = string
  description = "AWS secret key"
}

variable "region" {
  type    = string
  default = "us-east-2"
  description = "AWS region"
}

variable "instances_count" {
  type    = number
  default = 1
  description = "the count of normal instances"
}

variable "public_key_path" {
  type    = string
  description = "local path to ssh public key"
}

variable "private_key_path" {
  type    = string
  description = "local path to ssh private key"
}

variable "prefix" {
  type = string
  description = "prefix attach to resource names"
}

variable "instance_type" {
  type    = string
  default = "c5.xlarge"
}

variable "instance_type_bastion" {
  type    = string
  default = "t2.xlarge"
}

variable "username" {
  type    = string
  default = "ubuntu"
}

variable "private_ip_prefix" {
  type    = string
  default = "10.0.1"
}

variable "private_ip_bastion" {
  type    = string
  default = "10.0.1.10"
}
