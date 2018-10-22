# Regenerate structs_proto.rs

```sh
brew install protobuf
cargo install protobuf
protoc --rust_out . structs.proto
mv structs.rs ./src/structs_proto.rs
```
