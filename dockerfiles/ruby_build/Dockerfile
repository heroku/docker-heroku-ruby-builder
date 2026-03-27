ARG STACK_VERSION="24"
FROM heroku/heroku:${STACK_VERSION}-build

USER root

RUN apt-get update -y && apt-get install -y libreadline-dev ruby
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup update

WORKDIR /tmp/workdir/
COPY make_ruby.sh /tmp/workdir/make_ruby.sh
