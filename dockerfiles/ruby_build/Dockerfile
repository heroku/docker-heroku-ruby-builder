ARG STACK_VERSION="24"
FROM heroku/heroku:${STACK_VERSION}-build

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# This is a throwaway build image, not a runtime image. It intentionally stays
# root: the build writes into a host-owned bind mount (needs root on Linux) and
# rustup/cargo must be installed for the same user that runs the build.
# hadolint ignore=DL3002
USER root

# hadolint ignore=DL3008
RUN apt-get update -y \
    && apt-get install -y --no-install-recommends libreadline-dev ruby \
    && rm -rf /var/lib/apt/lists/*

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup update

WORKDIR /tmp/workdir/
COPY make_ruby.sh /tmp/workdir/make_ruby.sh
