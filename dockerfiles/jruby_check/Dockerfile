ARG STACK_VERSION="24"
FROM heroku/heroku:${STACK_VERSION}-build

# Only apt needs root: installing system packages writes to root-owned paths
USER root

# hadolint ignore=DL3008
RUN apt-get update -y; apt-get install default-jre default-jdk -y
USER heroku
