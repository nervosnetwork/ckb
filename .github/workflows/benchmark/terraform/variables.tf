/*
 * Input
 */

variable "access_key" {
  type = string
}

variable "secret_key" {
  type = string
}

variable "instances_count" {
  type    = number
  default = 2
}

/*
 * Pre-Generated Files
 */

variable "public_key_path" {
  type    = string
  default = "../keys/key.pub"
}

variable "private_key_path" {
  type    = string
  default = "../keys/key"
}

/*
 * Configuration of Machines
 */

variable "prefix" {
  type = string
}

variable "region" {
  type    = string
  default = "ap-northeast-1"
}

variable "instance_type_bastion" {
  type    = string
  default = "t2.xlarge"
}

variable "instance_type" {
  type    = string
  default = "c5.xlarge"
}

variable "username" {
  type    = string
  default = "ubuntu"
}

variable "upload_private_key_path" {
  type    = string
  default = "/home/ubuntu/.ssh/key"
}

variable "private_ip_prefix" {
  type    = string
  default = "10.0.1"
}

variable "private_ip_bastion" {
  type    = string
  default = "10.0.1.10"
}
