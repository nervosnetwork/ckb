#!/usr/bin/env python
# coding=utf-8

import os
import sys

def make_config():
    nid = int(sys.argv[2])
    path = os.path.join(sys.argv[1],"node" + str(nid))
    keypairs_path = os.path.join(sys.argv[1], "bls.keypairs")
    keypairs_f = open(keypairs_path, "r")
    keypairs = keypairs_f.readlines()
    config_name = "config"
    dump_path = os.path.join(path, config_name)
    f = open(dump_path, "w")
    key = keypairs[nid * 3]
    f.write("miner_private_key = " + key)
    f.write("signer_private_key = " + key + "\n")
    secret_path = os.path.join(path, "signer_privkey")
    f.write("[logger]" + "\n")
    f.write("file = \"/tmp/nervos.log\"\n")
    f.write("filter = \"main=info,miner=info,chain=info\"\n")
    f.write("color = true\n")
    secret_key = open(secret_path, "r")
    key = secret_key.read()
    secret_key.close()
    
    #generate keypairs
    signer_auth_path = os.path.join(sys.argv[1], "signer_authorities")
    signer_auth = open(signer_auth_path, "r")

    i = 1
    while True:
        signer_key = signer_auth.readline().strip('\n')
        proof_key = keypairs[i]
        proof_g = keypairs[i+1]
        if (not signer_key) or (not proof_key):
            break
        f.write("[[key_pairs]]" + "\n")
        f.write("proof_public_key = " + proof_key)
        f.write("proof_public_g = " + proof_g)
        f.write("signer_public_key = \"0x" + signer_key + "\"\n")
        i += 3

    signer_auth.close()
    keypairs_f.close()
    f.close()

make_config()
