## How to run

install component and tools (require rust nighlty)
```
rustup component add llvm-tools-preview
cargo install cargo-binutils
cargo install rustfilt
```

install cargo fuzz
```
cargo install cargo-fuzz
```

run fuzz test
```
cargo +nightly fuzz run transaction_scripts_verifier_data1
```

generate coverage report
```
cargo +nightly fuzz coverage transaction_scripts_verifier_data1
cargo +nightly cov -- show fuzz/target/target-tuples/release/transaction_scripts_verifier_data1 --Xdemangler=rustfilt --format=html -instr-profile=fuzz/coverage/transaction_scripts_verifier_data1/coverage.profdata --name=ckb --line-coverage-gt=1> /tmp/report.html
```
