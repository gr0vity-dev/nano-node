# syntax=docker/dockerfile:1.4
FROM ubuntu:22.04 AS builder

ARG COMPILER=gcc
ARG NANO_NETWORK=live

# Install build dependencies
COPY ./ci/prepare/linux /tmp/prepare
RUN /tmp/prepare/prepare.sh

# Define build arguments
ARG BUILD_TYPE=RelWithDebInfo
ARG NANO_TEST=OFF
ARG NANO_TRACING=ON
ARG COVERAGE=OFF
ARG CMAKE_SANITIZER=""
ARG CI_TAG=DEV_BUILD
ARG CI_VERSION_PRE_RELEASE=OFF
ARG SANITIZER

# Copy source and set workdir
COPY ./ /tmp/src
WORKDIR /tmp/src

# Use cache mount for build directory and copy outputs to a permanent location
RUN --mount=type=cache,target=/tmp/src/build,id=nano_build,sharing=locked \
    mkdir -p build && \
    ci/build-node.sh && \
    mkdir -p /tmp/build_output && \
    cp build/nano_node build/nano_rpc /tmp/build_output/

RUN echo ${NANO_NETWORK} >/etc/nano-network

FROM ubuntu:22.04

RUN groupadd --gid 1000 nanocurrency && \
    useradd --uid 1000 --gid nanocurrency --shell /bin/bash --create-home nanocurrency

# Copy from the permanent location instead of the cache mount
COPY --from=builder /tmp/build_output/nano_node /usr/bin
COPY --from=builder /tmp/build_output/nano_rpc /usr/bin
COPY --from=builder /tmp/src/api/ /usr/bin/api/
COPY --from=builder /etc/nano-network /etc
COPY docker/node/entry.sh /usr/bin/entry.sh
COPY docker/node/config /usr/share/nano/config

RUN chmod +x /usr/bin/entry.sh && \
    ln -s /usr/bin/nano_node /usr/bin/rai_node && \
    ldconfig

WORKDIR /root
USER root
ENV PATH="${PATH}:/usr/bin"

ENTRYPOINT ["/usr/bin/entry.sh"]
CMD ["nano_node", "daemon", "-l"]

ARG REPOSITORY=nanocurrency/nano-node
LABEL org.opencontainers.image.source https://github.com/$REPOSITORY