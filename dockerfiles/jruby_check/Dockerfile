ARG STACK_VERSION="24"
FROM heroku/heroku:${STACK_VERSION}-build

# Only apt needs root: installing system packages writes to root-owned paths
# hadolint ignore=DL3066
USER root

# hadolint ignore=DL3008
RUN apt-get update -y \
    && apt-get install -y --no-install-recommends default-jre default-jdk \
    && rm -rf /var/lib/apt/lists/*
# hadolint ignore=DL3066
USER heroku
