FROM fabiokung/cedar
MAINTAINER hone

# setup workspace
RUN rm -rf /tmp/workspace
RUN mkdir -p /tmp/workspace

# output dir is mounted

ADD build.rb /tmp/build.rb
CMD ["ruby", "/tmp/build.rb", "/tmp/workspace", "/tmp/output"]
