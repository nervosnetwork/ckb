FROM nervos/ckb-docker-builder:focal-rust-1.92.0 as ckb-docker-builder

WORKDIR /ckb
COPY ./ .

RUN make prod-docker

FROM ubuntu:focal
LABEL description="Nervos CKB is a public permissionless blockchain, the common knowledge layer of Nervos network."
LABEL maintainer="Nervos Core Dev <dev@nervos.org>"

RUN groupadd -g 1000 ckb \
 && useradd -m -u 1000 -g ckb -s /bin/sh ckb \
 && mkdir -p /var/lib/ckb

WORKDIR /var/lib/ckb

COPY --from=ckb-docker-builder \
     /usr/local/lib64/libssl.so.* \
     /usr/local/lib64/libcrypto.so.* \
     /usr/local/lib64/
COPY --from=ckb-docker-builder /ckb/target/prod/ckb /ckb/docker/docker-entrypoint.sh /bin/
RUN chown -R ckb:ckb /var/lib/ckb \
 && chmod 755 /var/lib/ckb

USER ckb
ENV CKB_CHAIN=mainnet
ENV LD_LIBRARY_PATH=/usr/local/lib64

EXPOSE 8114 8115
VOLUME ["/var/lib/ckb"]
ENTRYPOINT ["/bin/docker-entrypoint.sh"]
