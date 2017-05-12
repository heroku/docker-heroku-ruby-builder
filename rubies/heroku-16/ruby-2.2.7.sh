#!/bin/bash

source `dirname $0`/../common.sh

file_base=`basename "$0"` # for example: ruby-2.2.7.sh
ruby_version_from_filename=`echo "${file_base%.*}" | cut -d'-' -f 2` # for example: 2.2.7

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION="$ruby_version_from_filename"  -e STACK=heroku-16 hone/ruby-builder:heroku-16
