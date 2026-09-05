FROM ubuntu:22.04 AS ckb-docker-builder

ARG DEBIAN_FRONTEND=noninteractive
ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:${PATH}

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    curl \
    libclang-dev \
    libssl-dev \
    pkg-config \
 && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain 1.92.0 --profile minimal

WORKDIR /ckb
COPY ./ .

RUN make prod-docker

FROM ubuntu:22.04
LABEL description="Nervos CKB is a public permissionless blockchain, the common knowledge layer of Nervos network."
LABEL maintainer="Nervos Core Dev <dev@nervos.org>"

RUN groupadd -g 1000 ckb \
 && useradd -m -u 1000 -g ckb -s /bin/sh ckb \
 && mkdir -p /var/lib/ckb

WORKDIR /var/lib/ckb

COPY --from=ckb-docker-builder /ckb/target/prod/ckb /ckb/docker/docker-entrypoint.sh /bin/
RUN chown -R ckb:ckb /var/lib/ckb \
 && chmod 755 /var/lib/ckb

USER ckb
ENV CKB_CHAIN=mainnet

EXPOSE 8114 8115
VOLUME ["/var/lib/ckb"]
ENTRYPOINT ["/bin/docker-entrypoint.sh"]
