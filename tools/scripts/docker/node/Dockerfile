FROM ubuntu:22.04 AS builder

ARG COMPILER=gcc
ARG NANO_NETWORK=live

# Install build dependencies
COPY ./tools/scripts/ci/prepare/linux /tmp/prepare
RUN /tmp/prepare/prepare.sh

COPY ./ /tmp/src
WORKDIR /tmp/src

#Define ARGs for ci/build-node.sh
ARG BUILD_TYPE=RelWithDebInfo
ARG NANO_TEST=OFF
ARG NANO_TRACING=OFF
ARG COVERAGE=OFF
ARG CMAKE_SANITIZER=""
ARG CI_TAG=DEV_BUILD
ARG CI_VERSION_PRE_RELEASE=OFF
ARG SANITIZER

# Build node with cached cargo registries/target to avoid rebuilding unchanged deps
ENV PATH="/root/.cargo/bin:${PATH}"
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/tmp/src/target \
    cd main && CARGO_TARGET_DIR=/tmp/src/target cargo build --release
RUN echo ${NANO_NETWORK} >/etc/nano-network

FROM ubuntu:22.04

RUN groupadd --gid 1000 nanocurrency && \
    useradd --uid 1000 --gid nanocurrency --shell /bin/bash --create-home nanocurrency

COPY --from=builder /tmp/src/target/release/rsnano_node /usr/bin
COPY --from=builder /etc/nano-network /etc

COPY tools/scripts/docker/node/config /usr/share/nano/config
RUN ldconfig

WORKDIR /root
USER root

ENV PATH="${PATH}:/usr/bin"
ENTRYPOINT ["/usr/bin/rsnano_node"]
CMD ["node", "run"]

ARG REPOSITORY=simpago/rsnano-node
LABEL org.opencontainers.image.source=https://github.com/$REPOSITORY
