#!/bin/bash

source `dirname $0`/../common.sh

docker run -v `pwd`/cache:/tmp/cache -v `pwd`/builds:/tmp/output -e VERSION=1.9.2-p330 -e BUILD=true -e STACK=cedar hone/ruby-builder:cedar
