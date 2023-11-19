# CKB systemd unit configuration

The provided files should work with systemd version 219 or later.

## Instructions

The following instructions assume that:

* you want to run ckb as user `ckb` and group `ckb`, and store data in `/var/lib/ckb`.
* you want to join mainnet.
* you are logging in as a non-root user account that has `sudo` permissions to execute commands as root.

First, get ckb and move the binary into the system binary directory, and setup the appropriate ownership and permissions:

```bash
sudo cp /path/to/ckb /usr/local/bin
sudo chown root:root /usr/local/bin/ckb
sudo chmod 755 /usr/local/bin/ckb
```

Setup the directories and generate config files for mainnet.

```bash
sudo mkdir /var/lib/ckb
sudo /usr/local/bin/ckb init -C /var/lib/ckb --chain mainnet --log-to stdout
```

Setup the user and group and the appropriate ownership and permissions.

```bash
sudo groupadd ckb
sudo useradd \
  -g ckb --no-user-group \
  --home-dir /var/lib/ckb --no-create-home \
  --shell /usr/sbin/nologin \
  --system ckb

sudo chown -R ckb:ckb /var/lib/ckb
sudo chmod 755 /var/lib/ckb
sudo chmod 644 /var/lib/ckb/ckb.toml /var/lib/ckb/ckb-miner.toml
```

Install the systemd service unit configuration file, reload the systemd daemon,
and start the node:

```bash
curl -L -O https://raw.githubusercontent.com/nervosnetwork/ckb/master/devtools/init/linux-systemd/ckb.service
sudo cp ckb.service /etc/systemd/system/
sudo chown root:root /etc/systemd/system/ckb.service
sudo chmod 644 /etc/systemd/system/ckb.service
sudo systemctl daemon-reload
sudo systemctl start ckb.service
```

Start the node automatically on boot if you like:

```bash
sudo systemctl enable ckb.service
```

Check ckb's status:

```bash
sudo systemctl status ckb.service
```

If ckb doesn't seem to start properly you can view the logs to figure out the problem:

```bash
sudo journalctl --boot -u ckb.service
```

Following the similar instructions to start a miner:

```bash
curl -L -O https://raw.githubusercontent.com/nervosnetwork/ckb/master/devtools/init/linux-systemd/ckb-miner.service
sudo cp ckb-miner.service /etc/systemd/system/
sudo chown root:root /etc/systemd/system/ckb-miner.service
sudo chmod 644 /etc/systemd/system/ckb-miner.service
sudo systemctl daemon-reload
sudo systemctl start ckb-miner.service
```

Let the miner starts automatically on boot:

```bash
sudo systemctl enable ckb-miner.service
```
