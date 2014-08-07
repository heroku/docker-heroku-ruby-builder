#!/bin/bash

source `dirname $0`/common.sh

docker.io run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.1.0 hone/ruby-builder:14
