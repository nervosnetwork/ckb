variable "access_key" {
  type = string
}

variable "secret_key" {
  type = string
}

variable "public_key_path" {
  type    = string
}

variable "private_key_path" {
  type    = string
}

variable "prefix" {
  type = string
}

variable "instance_type" {
  type = string
    default = "t3.micro"
}

variable "username" {
  type    = string
  default = "ubuntu"
}
