variable "access_key" {
  type = string
  description = "AWS access key"
}

variable "secret_key" {
  type = string
  description = "AWS secret key"
}

variable "public_key_path" {
  type    = string
  description = "local path to ssh public key file"
}

variable "private_key_path" {
  type    = string
  description = "local path to ssh private key file"
}

variable "prefix" {
  type = string
  description = "prefix attached to resources names"
}

variable "instance_type" {
  type = string
  default = "t3.micro"
  description = "instance type"
}

variable "username" {
  type    = string
  default = "ubuntu"
}
