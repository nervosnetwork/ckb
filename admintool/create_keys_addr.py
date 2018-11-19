import os
import copy
import sys
import random
from secp256k1 import PrivateKey, PublicKey
import rlp
from rlp.utils import decode_hex, encode_hex 
from utils import privtopub

def mk_privkey(seed):
    return sha3(seed)

def mk_miner_keys_addr():
    if len(sys.argv)==2:
        path = sys.argv[1]
    else:
        path = os.path.join(sys.argv[1],"node" + sys.argv[2])
    dump_path = os.path.join(path, "miner_privkey")
    privkey = PrivateKey() 
    sec_key = privkey.serialize()
    f = open(dump_path, "w")
    f.write(sec_key)
    f.close()
    auth_path = os.path.join(sys.argv[1], "miner_authorities")
    authority = encode_hex(privtopub(decode_hex(sec_key)))[2:]
    auth_file = open(auth_path, "a")
    auth_file.write(authority + "\n")
    auth_file.close()

def mk_signer_keys_addr():
    if len(sys.argv)==2:
        path = sys.argv[1]
    else:
        path = os.path.join(sys.argv[1],"node" + sys.argv[2])
    dump_path = os.path.join(path, "signer_privkey")
    privkey = PrivateKey() 
    sec_key = privkey.serialize()
    f = open(dump_path, "w")
    f.write(sec_key)
    f.close()
    auth_path = os.path.join(sys.argv[1], "signer_authorities")
    authority = encode_hex(privtopub(decode_hex(sec_key)))[2:]
    auth_file = open(auth_path, "a")
    auth_file.write(authority + "\n")
    auth_file.close()

# mk_miner_keys_addr()
mk_signer_keys_addr()
