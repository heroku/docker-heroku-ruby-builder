#!/bin/bash

source `dirname $0`/common.sh

docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.1.2 -e SVN_URL=http://svn.ruby-lang.org/repos/ruby/trunk -e RELNAME=branches/ruby_2_1@47326 hone/ruby-builder:cedar

