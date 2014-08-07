#!/bin/bash

source `dirname $0`/common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.0.0-p481 hone/ruby-builder:cedar
