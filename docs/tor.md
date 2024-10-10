# Running CKB Node Behind Tor Hidden Network

1. make sure tor server is running and listening. (By default tor will listening on port 9050 )

You can get a default torrc file by searching `--torrc-file` in `man tor`, it will tell you where the default torrc file location in your OS.

```
       -f, --torrc-file FILE
           Specify a new configuration file to contain further Tor configuration options, or pass - to make
           Tor read its configuration from standard input. (Default:
           /nix/store/gacp048mx3m4q48gl5rlspw2j33328v4-tor-0.4.8.11/etc/tor/torrc, or $HOME/.torrc if that
           file is not found.)
```
You may need start tor service by your owned `torrc`

```bash
# Start tor service
tor -f /path/to/your/torrc

```
Check tor is listening on 9050:

```bash
$ netstat -anlp | grep -i tor
(Not all processes could be identified, non-owned process info
 will not be shown, you would have to be root to see it all.)
tcp        0      0 127.0.0.1:9050          0.0.0.0:*               LISTEN      969485/tor
tcp        0      0 127.0.0.1:9051          0.0.0.0:*               LISTEN      969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:56078         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:47818         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:57642         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:47868         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:54598         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:47844         ESTABLISHED 969485/tor
tcp        0      0 28.0.0.1:43478          213.32.104.213:9100     ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:47830         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:46074         ESTABLISHED 969485/tor
tcp        0      0 28.0.0.1:52502          217.12.203.196:9001     ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:55958         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:47886         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:58530         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:54610         ESTABLISHED 969485/tor
tcp        0      0 127.0.0.1:9050          127.0.0.1:47876         ESTABLISHED 969485/tor
unix  2      [ ]         DGRAM      CONNECTED     65129318 969485/tor

```
2. start ckb node by proxychains-ng, (ckb need proxychains to proxy all network traffic to tor server)
In proxychains-ng''s  configuration, let it proxy trafiic to 9050
```bash
proxychains4 ckb run
```

3. test connection to Tor Hidden Network
```bash

# check if you can access duckduckgo's onion service
curl -x socks5h://127.0.0.1:9050 https://duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion

# check if the response tell you IsTor: true ?
curl -x socks5h://127.0.0.1:9050 -s https://check.torproject.org/api/ip

```
TODO: get bridge from https://bridges.torproject.org/options

Config `UseBridges` `ClientTransportPlugin` related config in `torrc` file.

4. view tor's log (may need to enable more verbose log in tor's torrc config file)
