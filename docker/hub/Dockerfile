FROM nervos/ckb-docker-builder:bionic-rust-1.46.0 as ckb-docker-builder

WORKDIR /ckb
COPY ./ .

RUN make prod-docker

FROM ubuntu:bionic
LABEL description="Nervos CKB is a public permissionless blockchain, the common knowledge layer of Nervos network."
LABEL maintainer="Nervos Core Dev <dev@nervos.org>"

RUN groupadd -g 1000 ckb \
 && useradd -m -u 1000 -g ckb -s /bin/sh ckb \
 && mkdir -p /var/lib/ckb

WORKDIR /var/lib/ckb

COPY --from=ckb-docker-builder \
     /usr/lib/x86_64-linux-gnu/libssl.so.* \
     /usr/lib/x86_64-linux-gnu/libcrypto.so.* \
     /usr/lib/x86_64-linux-gnu/
COPY --from=ckb-docker-builder /ckb/target/release/ckb /ckb/docker/docker-entrypoint.sh /bin/
RUN chown -R ckb:ckb /var/lib/ckb \
 && chmod 755 /var/lib/ckb

USER ckb
ENV CKB_CHAIN=mainnet

EXPOSE 8114 8115
VOLUME ["/var/lib/ckb"]
ENTRYPOINT ["/bin/docker-entrypoint.sh"]
