#!/bin/bash

if [ -d "/root/mount" ]; then
	cp -a "/root/mount/." "/root/work"
fi

if [ -f "/root/work/dockerfiles/support/Makefile.docker" ]; then
	cp "/root/work/dockerfiles/support/Makefile.docker" "/root/work/Makefile"
fi

exec "$@"
