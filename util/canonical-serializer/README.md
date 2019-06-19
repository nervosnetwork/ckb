# Canonical Serializer

fixed length encoding

* u8  - 1 bytes little endian encode
* u32 - 4 bytes little endian encode
* u64 - 8 bytes little endian encode
* u256 - 32 bytes little endian encode
* H160 - 20 bytes raw binary
* H256 - 32 bytes raw binary

``` python
def encode_u8(n):
    return n.to_bytes(1, 'little', signed=False)

def encode_u32(n):
    return n.to_bytes(4, 'little', signed=False)

def encode_u64(n):
    return n.to_bytes(8, 'little', signed=False)

```

variable length encoding

for a variable length data, we encode the length as u32, then concat the data it self, 
if the data is a list, we encode each element then concat them into bytes.

``` python
def encode_bytes(bytes):
    length = encode_u32(len(bytes))
    return length + bytes

# encode_bytes(b"hello world") => b'\x0b\x00\x00\x00hello world'
```

``` python
# ignore int_type when elem is not a int, otherwise use 8, 32 or 64.
def encode_list(lst, int_type=0):
    length = encode_u32(len(lst))
    return length + b"".join([encode_elem(elem, int_type=int_type) for elem in lst])   

def encode_elem(item, int_type=0):
    if isinstance(item, bytes):
        return encode_bytes(item)
    elif isinstance(item, int):
        assert int_type % 8 == 0
        bytes_length = int_type // 8
        assert bytes_length > 0
        return item.to_bytes(bytes_length, 'little', signed=False)
    
    raise Exception("not support nested list in this demo")

# encode_list([1, 2, 3], int_type=8) => b'\x03\x00\x00\x00\x01\x02\x03'
# encode_list([b"hello", b"world", b"blockchain"]) => 
# b'\x03\x00\x00\x00\x05\x00\x00\x00hello\x05\x00\x00\x00world\n\x00\x00\x00blockchain'
```

advance types:

optional item - optional item can encoding as `prefix + encode(item)`, when item exists prefix is `\x01`, otherwise it is `\x00`.
For example a optional u64 item: `encode_u8(1) + encode_u64(42)`

