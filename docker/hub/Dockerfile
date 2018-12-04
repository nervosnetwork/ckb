FROM ubuntu:bionic
LABEL description="Nervos CKB is a public permissionless blockchain, the common knowledge layer of Nervos network."
LABEL maintainer="Nervos Core Dev <dev@nervos.org>"

#build arg
ARG DATADIR=/usr/local/share/ckb
ENV DATADIR ${DATADIR}

#useradd
RUN groupadd -g 1000 ckb && useradd -m -u 1000 -g ckb -s /bin/sh ckb

WORKDIR ${DATADIR}

#apt
RUN apt update && apt install -y --no-install-recommends openssl1.0 libssl1.0

#cleanup
RUN apt autoremove -y
RUN apt clean -y
RUN rm -rf /tmp/* /var/tmp/* /var/lib/apt/lists/*

#target
COPY target/release/ckb /bin/ckb

#entrypoint
RUN echo "#!/bin/bash \n ckb \$@" > ./entrypoint.sh
RUN chmod +x ./entrypoint.sh

# copy "nodes_template" to `WORKDIR`/nodes/
COPY nodes_template/ nodes/
RUN chown -R ckb:ckb $DATADIR/nodes

#switch use
USER ckb

EXPOSE 8114/tcp 8115/udp
ENTRYPOINT ["./entrypoint.sh"]
