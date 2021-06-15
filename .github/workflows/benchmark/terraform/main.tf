provider "aws" {
  region     = var.region
  access_key = var.access_key
  secret_key = var.secret_key
}

resource "aws_vpc" "vpc" {
  cidr_block           = "10.0.0.0/16"
  enable_dns_hostnames = true
  tags = {
    Name = "${var.prefix}-vpc"
  }
}

resource "aws_subnet" "subnet" {
  vpc_id                  = aws_vpc.vpc.id
  cidr_block              = "${var.private_ip_prefix}.0/24"
  map_public_ip_on_launch = true
  tags = {
    Name = "${var.prefix}-subnet"
  }
}

resource "aws_internet_gateway" "ig" {
  vpc_id = aws_vpc.vpc.id
  tags = {
    Name = "${var.prefix}-ig"
  }
}

resource "aws_route" "internet_access" {
  route_table_id         = aws_vpc.vpc.main_route_table_id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.ig.id
}

resource "aws_key_pair" "ssh" {
  key_name   = "${var.prefix}-ssh_key"
  public_key = file(var.public_key_path)
}

resource "aws_security_group" "sg_bastion" {
  name   = "${var.prefix}-sg_bastion"
  vpc_id = aws_vpc.vpc.id

  ingress {
    description = "ssh"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "all inbound from anywhere"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "all outbound to anywhere"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_instance" "bastion" {
  key_name               = aws_key_pair.ssh.id
  instance_type          = var.instance_type_bastion
  ami                    = data.aws_ami.ubuntu.id
  vpc_security_group_ids = [aws_security_group.sg_bastion.id]
  private_ip             = var.private_ip_bastion
  subnet_id              = aws_subnet.subnet.id

  root_block_device {
    volume_size = "60"
  }

  connection {
    type        = "ssh"
    host        = aws_instance.bastion.public_ip
    user        = var.username
    private_key = file(var.private_key_path)
  }

  provisioner "remote-exec" {
    inline = [
      "while [ ! -f /var/lib/cloud/instance/boot-finished ]; do echo 'Waiting for cloud-init...'; sleep 1; done",
    ]
  }

  tags = {
    Name = "${var.prefix}-bastion"
  }
}

resource "aws_security_group" "sg_default" {
  name        = "${var.prefix}-sg-default"
  description = "Allow inbound access from anywhere and outbound access to all"
  vpc_id      = aws_vpc.vpc.id

  ingress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_instance" "instance" {
  count                  = var.instances_count
  key_name               = aws_key_pair.ssh.id
  instance_type          = var.instance_type
  ami                    = data.aws_ami.ubuntu.id
  vpc_security_group_ids = [aws_security_group.sg_default.id]
  private_ip             = "${var.private_ip_prefix}.${count.index + 100}"
  subnet_id              = aws_subnet.subnet.id

  root_block_device {
    volume_size = "60"
  }

  tags = {
    Name  = "${var.prefix}-instance-${count.index}"
    Index = "${count.index}"
  }
}

resource "null_resource" "instance_provisioners" {
  count = var.instances_count

  triggers = {
    cluster_instance_ids = join(",", aws_instance.instance.*.id)
  }

  connection {
    bastion_host = aws_instance.bastion.public_ip
    host         = element(aws_instance.instance.*.private_ip, count.index)
    user         = var.username
    private_key  = file(var.private_key_path)
  }

  provisioner "remote-exec" {
    inline = [
      "while [ ! -f /var/lib/cloud/instance/boot-finished ]; do echo 'Waiting for cloud-init...'; sleep 1; done",
    ]
  }
}

output "ansible_hosts" {
  value = <<EOF

bastions:
  hosts:
    bastion-0:
      ansible_host: ${aws_instance.bastion.public_ip}
      instance_type: ${aws_instance.bastion.instance_type}
      ansible_user: ${var.username}

instances:
  hosts:
${join(
"\n",
formatlist(
  "    instance-%s:\n      ansible_host: %s\n      instance_type: %s\n      ansible_user: %s",
  aws_instance.instance.*.tags.Index,
  aws_instance.instance.*.public_ip,
  aws_instance.instance.*.instance_type,
  var.username
)
)}
EOF
}
