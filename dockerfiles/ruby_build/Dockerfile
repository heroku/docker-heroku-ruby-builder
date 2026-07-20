ARG STACK_VERSION="24"
FROM heroku/heroku:${STACK_VERSION}-build

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Only apt needs root: installing system packages writes to root-owned paths
USER root
RUN apt-get update -y && apt-get install -y libreadline-dev ruby
USER heroku

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/home/heroku/.cargo/bin:${PATH}"
RUN rustup update

WORKDIR /tmp/workdir/
COPY make_ruby.sh /tmp/workdir/make_ruby.sh
