provider "aws" {
    region = "ap-northeast-1"
    alias  = "ap-northeast-1"
    access_key = "${var.access_key}"
    secret_key = "${var.secret_key}"
}

data "aws_ami" "ubuntu-ap-northeast-1" {
  provider = aws.ap-northeast-1
  most_recent = true
  owners      = ["099720109477"] # Canonical
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd/ubuntu-focal-20.04-amd64-server-*"]
  }
}

resource "aws_key_pair" "key-ap-northeast-1" {
  provider      = aws.ap-northeast-1
  key_name   = "${var.prefix}"
  public_key = file(var.public_key_path)
}

resource "aws_instance" "instance-ap-northeast-1" {
  provider = aws.ap-northeast-1
  ami           = data.aws_ami.ubuntu-ap-northeast-1.id
  key_name      = aws_key_pair.key-ap-northeast-1.id
  instance_type = "${var.instance_type}"
  root_block_device {
    volume_size = "60"
  }
  tags = {
    Name  = "${var.prefix}"
  }

  provisioner "remote-exec" {
    inline = [
      "while [ ! -f /var/lib/cloud/instance/boot-finished ]; do echo 'Waiting for cloud-init...'; sleep 1; done",
    ]

    connection {
        type        = "ssh"
        host        = aws_instance.instance-ap-northeast-1.public_ip
        user        = var.username
        private_key = file(var.private_key_path)
    }
  }
}

output "ansible-hosts-ap-northeast-1" {
  value = <<EOF

ap-northeast-1:
  hosts:
    instance-ap-northeast-1:
      ansible_host: ${aws_instance.instance-ap-northeast-1.public_ip}
      instance_type: ${aws_instance.instance-ap-northeast-1.instance_type}
      region: ap-southeast-1
EOF
}
provider "aws" {
    region = "ap-southeast-1"
    alias  = "ap-southeast-1"
    access_key = "${var.access_key}"
    secret_key = "${var.secret_key}"
}

data "aws_ami" "ubuntu-ap-southeast-1" {
  provider = aws.ap-southeast-1
  most_recent = true
  owners      = ["099720109477"] # Canonical
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd/ubuntu-focal-20.04-amd64-server-*"]
  }
}

resource "aws_key_pair" "key-ap-southeast-1" {
  provider      = aws.ap-southeast-1
  key_name   = "${var.prefix}"
  public_key = file(var.public_key_path)
}

resource "aws_instance" "instance-ap-southeast-1" {
  provider = aws.ap-southeast-1
  ami           = data.aws_ami.ubuntu-ap-southeast-1.id
  key_name      = aws_key_pair.key-ap-southeast-1.id
  instance_type = "${var.instance_type}"
  root_block_device {
    volume_size = "60"
  }
  tags = {
    Name  = "${var.prefix}"
  }

  provisioner "remote-exec" {
    inline = [
      "while [ ! -f /var/lib/cloud/instance/boot-finished ]; do echo 'Waiting for cloud-init...'; sleep 1; done",
    ]

    connection {
        type        = "ssh"
        host        = aws_instance.instance-ap-southeast-1.public_ip
        user        = var.username
        private_key = file(var.private_key_path)
    }
  }
}

output "ansible-hosts-ap-southeast-1" {
  value = <<EOF

ap-southeast-1:
  hosts:
    instance-ap-southeast-1:
      ansible_host: ${aws_instance.instance-ap-southeast-1.public_ip}
      instance_type: ${aws_instance.instance-ap-southeast-1.instance_type}
      region: ap-southeast-1
EOF
}
