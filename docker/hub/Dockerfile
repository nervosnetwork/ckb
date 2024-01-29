FROM nervos/ckb-docker-builder:bionic-rust-1.71.1-openssl-3.1.3 as ckb-docker-builder

WORKDIR /ckb
COPY ./ .

RUN make OPENSSL_STATIC=1 OPENSSL_LIB_DIR=/usr/local/lib64 OPENSSL_INCLUDE_DIR=/usr/local/include prod-docker

FROM ubuntu:bionic
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
