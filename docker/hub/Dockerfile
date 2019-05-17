FROM nervos/ckb-docker-builder:xenial-rust-1.34.2 as ckb-docker-builder

WORKDIR /ckb
COPY ./ .

RUN make VERBOSE=--verbose prod

FROM ubuntu:xenial
LABEL description="Nervos CKB is a public permissionless blockchain, the common knowledge layer of Nervos network."
LABEL maintainer="Nervos Core Dev <dev@nervos.org>"

RUN groupadd -g 1000 ckb \
 && useradd -m -u 1000 -g ckb -s /bin/sh ckb \
 && mkdir -p /var/lib/ckb \
 && chown -R ckb:ckb /var/lib/ckb \
 && chmod 755 /var/lib/ckb

WORKDIR /var/lib/ckb

COPY --from=ckb-docker-builder \
     /lib/x86_64-linux-gnu/libssl.so.* \
     /lib/x86_64-linux-gnu/libcrypto.so.* \
     /lib/x86_64-linux-gnu/
COPY --from=ckb-docker-builder /ckb/target/release/ckb /bin/ckb
RUN /bin/ckb init --force

USER ckb

EXPOSE 8114 8115
VOLUME ["/var/lib/ckb"]
ENTRYPOINT ["/bin/ckb"]
