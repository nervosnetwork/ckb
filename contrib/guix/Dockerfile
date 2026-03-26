# Derived from fanquake's Guix Docker setup for Bitcoin Core.
# Reference: https://github.com/fanquake/core-review/tree/master/guix
FROM alpine:3.23

RUN apk --no-cache add bash bzip2 ca-certificates curl git make shadow xz

# Install Guix 1.5.0 binary release.
# Verify the SHA256 checksum to ensure integrity.
ENV GUIX_VERSION=1.5.0
RUN cd /tmp \
    && curl -fsSLO "https://ftp.gnu.org/gnu/guix/guix-binary-${GUIX_VERSION}.x86_64-linux.tar.xz" \
    && echo "aa41025489c5061543e9c48873eaa829b900b2da75d40f9648913622f5f47817  guix-binary-${GUIX_VERSION}.x86_64-linux.tar.xz" \
       | sha256sum -c - \
    && tar xJf "guix-binary-${GUIX_VERSION}.x86_64-linux.tar.xz" -C / \
    && rm "guix-binary-${GUIX_VERSION}.x86_64-linux.tar.xz" \
    && mkdir -p /root/.config/guix \
    && ln -sf /var/guix/profiles/per-user/root/current-guix /root/.config/guix/current

ENV PATH="/root/.config/guix/current/bin:${PATH}"

# Create 32 build users for guix-daemon's sandboxed builds.
RUN groupadd --system guixbuild \
    && for i in $(seq -w 1 32); do \
         useradd -g guixbuild -G guixbuild \
           -d /var/empty \
           -s "$(command -v nologin)" \
           -c "Guix build user $i" \
           --system "guixbuilder$i"; \
       done

# Authorize the official Guix substitute servers so packages can be
# downloaded as pre-built binaries instead of compiled from source.
RUN guix archive --authorize < /root/.config/guix/current/share/guix/ci.guix.gnu.org.pub \
    && guix archive --authorize < /root/.config/guix/current/share/guix/bordeaux.guix.gnu.org.pub

# The daemon runs as the container's main process (PID 1).
# Builds are executed via `docker exec`.
CMD ["guix-daemon", "--build-users-group=guixbuild"]
