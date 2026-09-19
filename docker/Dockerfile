# docker/Dockerfile — Alter Zero on headless Kali Linux (docs/docker.md).
#
# Build it with docker/build.sh, which resolves the latest release first:
#
#   docker/build.sh                      # Docker
#   docker/build.sh --engine podman      # Podman
#
# Nothing is compiled here. The binary is the published release, downloaded and
# SHA-256-verified by the repository's own install.sh, so the image carries no
# Rust toolchain and no source tree. The build context is the repository root,
# cut down to three files by the allowlist in /.dockerignore.
ARG KALI_IMAGE=docker.io/kalilinux/kali-rolling:latest

# ---------------------------------------------------------------------------
# tools — the base every later stage starts from. It is the slow layer, so it
# comes first: a new Alter Zero release rebuilds only the stages below it.
# ---------------------------------------------------------------------------
FROM ${KALI_IMAGE} AS tools

# No desktop, no Kali metapackage, no compiler, and no recommended packages:
# each tool here is either small or something the agent cannot work without
# (git is what /diff and the checkpoints shell out to). Measured sizes, and
# what was left out and why, are in docs/docker.md.
#
# The dpkg excludes come first, debian:slim's own technique: translations, man
# pages (there is no `man` here to read them) and package docs are never
# unpacked — 47 MB of this layer otherwise. Each package's copyright file is
# kept. They stay in force for what you `apt install` later; delete the file
# and reinstall a package to get its pages back.
#
# The setcap line: Kali ships the scanner with forced file capabilities
# (net_admin among them). A container's default set has no NET_ADMIN, and the
# kernel then refuses to exec the binary at all — "Operation not permitted",
# as root too. Without them root simply uses whatever the runtime grants
# (docker/run.sh adds NET_RAW, which is what a SYN scan needs). A later
# `apt upgrade` would put them back, so the dpkg hook written beside it strips
# them again after every package run.
#
# `setcap -r` exits 1 when there is nothing to remove, so it is tolerated
# rather than chained strictly: on a future Kali that ships the scanner
# without those capabilities this line becomes a no-op, and failing the whole
# build at the moment its own reason disappeared would be absurd. Nothing is
# silently lost — smoke_image.py asserts the capabilities are gone *and* that
# the binary runs, so a real regression still fails, in the test that owns it.
RUN printf '%s\n' \
        '# Alter Zero container (docker/Dockerfile): a headless image reads none of these.' \
        'path-exclude /usr/share/doc/*' \
        'path-include /usr/share/doc/*/copyright' \
        'path-exclude /usr/share/man/*' \
        'path-exclude /usr/share/info/*' \
        'path-exclude /usr/share/lintian/*' \
        'path-exclude /usr/share/locale/*' \
        'path-include /usr/share/locale/locale.alias' \
        > /etc/dpkg/dpkg.cfg.d/90alter-zero-slim \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl wget git openssh-client \
        jq ripgrep file less nano tree xxd unzip \
        python3 procps tini libcap2-bin \
        net-tools iproute2 iputils-ping traceroute \
        nmap whois netcat-openbsd socat bind9-dnsutils openssl \
        kitty-terminfo ncurses-term \
    && { setcap -r /usr/lib/nmap/nmap 2>/dev/null || true; } \
    && printf '%s\n' \
        '// Alter Zero container (docker/Dockerfile): keep the scanner runnable.' \
        'DPkg::Post-Invoke { "if [ -x /usr/lib/nmap/nmap ]; then setcap -r /usr/lib/nmap/nmap 2>/dev/null || true; fi"; };' \
        > /etc/apt/apt.conf.d/90alter-zero-nmap-caps \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Anything else you want baked in: docker/build.sh --with "binutils tcpdump".
# Its own layer, after the tools, so adding a package never reinstalls them.
ARG EXTRA_PACKAGES=""
RUN if [ -n "$EXTRA_PACKAGES" ]; then \
        apt-get update \
        && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $EXTRA_PACKAGES \
        && apt-get clean \
        && rm -rf /var/lib/apt/lists/*; \
    fi

# ---------------------------------------------------------------------------
# release — fetch the published binary. A stage of its own, so the installer
# and its temporary files never reach the final image.
# ---------------------------------------------------------------------------
FROM tools AS release

# docker/build.sh passes the tag it resolved, which is what makes this layer
# rebuild exactly when a new release exists. Left empty (a bare `docker
# build`), install.sh resolves "latest" itself — and the layer cache then keeps
# serving that answer until you pass --no-cache. An empty base URL is
# install.sh's own default, github.com; a fork points it at its own releases.
ARG ALTER_ZERO_VERSION=""
ARG ALTER_ZERO_INSTALL_BASE_URL=""
COPY install.sh /tmp/install.sh
RUN ALTER_ZERO_INSTALL_DIR=/opt/release \
    ALTER_ZERO_VERSION="$ALTER_ZERO_VERSION" \
    ALTER_ZERO_INSTALL_BASE_URL="$ALTER_ZERO_INSTALL_BASE_URL" \
    sh /tmp/install.sh

# ---------------------------------------------------------------------------
# The image.
# ---------------------------------------------------------------------------
FROM tools

ARG ALTER_ZERO_VERSION=""
LABEL org.opencontainers.image.title="Alter Zero · headless Kali" \
      org.opencontainers.image.description="Alter Zero's published release on Kali Rolling, with a small set of command-line tools" \
      org.opencontainers.image.source="https://github.com/linuztx/alter-zero" \
      org.opencontainers.image.documentation="https://github.com/linuztx/alter-zero/blob/main/docker/README.md" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${ALTER_ZERO_VERSION}"

COPY --from=release /opt/release/alter-zero /usr/local/bin/alter-zero
COPY LICENSE TELEMETRY.md /usr/share/doc/alter-zero/

# /workspace is where you work; /run/alter-zero is where docker/run.sh
# --clipboard mounts the desktop's sockets (a runtime dir wants mode 0700).
RUN mkdir -p /workspace && mkdir -m 0700 /run/alter-zero

# Defaults only. `docker exec -e TERM -e COLORTERM …` hands over your real
# terminal's values per session, which is what picks the image protocol.
#
# Telemetry is deliberately left alone: nothing here sets ALTER_ZERO_TELEMETRY
# or DO_NOT_TRACK, so the app behaves exactly as an ordinary install does —
# its first-run notice, its daily ping, its own off switches (TELEMETRY.md).
ENV LANG=C.UTF-8 \
    TERM=xterm-256color \
    COLORTERM=truecolor

# Root, as Kali's own image is: no user is added.
WORKDIR /workspace

# Nothing listens by default. These are the two ports docker/run.sh publishes
# to your machine, for whatever you or the agent start on them.
EXPOSE 8080 8888

# Idle under an init that reaps children, so the container is a place to
# `exec` into — once for alter-zero, again for a shell — rather than one
# program's lifetime.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["sleep", "infinity"]
