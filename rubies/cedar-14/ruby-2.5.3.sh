#!/bin/bash

source `dirname $0`/../common.sh

VERSION=2.5.3  STACK=cedar-14 ruby build.rb /tmp/workspace $OUTPUT_DIR $CACHE_DIR
