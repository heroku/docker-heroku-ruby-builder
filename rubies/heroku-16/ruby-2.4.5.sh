#!/bin/bash

source `dirname $0`/../common.sh

docker run -v $OUTPUT_DIR:/tmp/output \
           -v $CACHE_DIR:/tmp/cache \
           -e VERSION=2.4.5 \
           -e STACK=heroku-16 \
           -e PATCH_URL=https://gist.githubusercontent.com/schneems/fd2bd841515367871e5b332afe9455ea/raw/1de62a84bce330c9ca8336fddb7c143c36a01a1d/ruby-2.4.5-rubygems.patch \
           hone/ruby-builder:heroku-16
