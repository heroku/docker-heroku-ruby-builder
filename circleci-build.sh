#!/usr/bin/env bash

set -o pipefail
set -eu

RUBY_VERSION="${1:?}"
CLEAR_TEXT_CIRCLECI_TOKEN="$(echo "${CIRCLECI_TOKEN:?}" | gpg --decrypt)"

curl -X POST \
  --header "Content-Type: application/json" \
  -d "{\"name\":\"RUBY_VERSION\", \"value\":\"${RUBY_VERSION}\"}" \
  "https://circleci.com/api/v1.1/project/github/heroku/docker-heroku-ruby-builder/envvar?circle-token=${CLEAR_TEXT_CIRCLECI_TOKEN}"

curl -X POST \
  --header "Content-Type: application/json" \
  -d '{"branch": "master"}' \
  "https://circleci.com/api/v1.1/project/github/heroku/docker-heroku-ruby-builder/build?circle-token=${CLEAR_TEXT_CIRCLECI_TOKEN}"
