FROM ubuntu:bionic as ckb-builder

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    RUSTUP_VERSION=1.18.1 \
    RUSTUP_SHA256=7d610882f67ec4e53a56a8177d7862501a043f80be7ba13b6e325cc9921f23b8 \
    RUST_ARCH=x86_64-unknown-linux-gnu \
    RUST_VERSION=1.34.1

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        gcc \
        libc6-dev \
        wget \
        libssl-dev \
        git \
        pkg-config \
        libclang-dev clang

RUN url="https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${RUST_ARCH}/rustup-init"; \
    wget "$url"; \
    echo "${RUSTUP_SHA256} *rustup-init" | sha256sum -c -; \
    chmod +x rustup-init

RUN ./rustup-init -y --no-modify-path --default-toolchain $RUST_VERSION; \
    rm rustup-init; \
    chmod -R a+w $RUSTUP_HOME $CARGO_HOME; \
    rustup --version; \
    cargo --version; \
    rustc --version; \
    openssl version;

WORKDIR /ckb
COPY ./ .

RUN make VERBOSE=--verbose prod


FROM ubuntu:bionic
LABEL description="Nervos CKB is a public permissionless blockchain, the common knowledge layer of Nervos network."
LABEL maintainer="Nervos Core Dev <dev@nervos.org>"

RUN groupadd -g 1000 ckb \
 && useradd -m -u 1000 -g ckb -s /bin/sh ckb \
 && mkdir -p /var/lib/ckb \
 && chown -R ckb:ckb /var/lib/ckb \
 && chmod 755 /var/lib/ckb

WORKDIR /var/lib/ckb

COPY --from=ckb-builder \
     /usr/lib/x86_64-linux-gnu/libssl.so.* \
     /usr/lib/x86_64-linux-gnu/libcrypto.so.* \
     /usr/lib/x86_64-linux-gnu/
COPY --from=ckb-builder /ckb/target/release/ckb /bin/ckb
RUN /bin/ckb init --force

USER ckb

EXPOSE 8114 8115
VOLUME ["/var/lib/ckb"]
ENTRYPOINT ["/bin/ckb"]
