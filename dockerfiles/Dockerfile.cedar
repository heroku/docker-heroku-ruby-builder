FROM fabiokung/cedar
MAINTAINER hone

# need autoconf 2.69 for compiling ruby 2.1+
RUN apt-get install subversion zip -y
# need bison 2.7 for compiling ruby 1.9.2
RUN cd /tmp/ && curl -O http://ftp.gnu.org/gnu/bison/bison-2.7.tar.gz && tar xzf bison-2.7.tar.gz && cd bison-2.7 && ./configure && make install && cd .. && rm -rf bison-2.7 && rm bison-2.7.tar.gz
# need autoconf 2.67
RUN cd /tmp/ && curl -O http://ftp.gnu.org/gnu/autoconf/autoconf-2.67.tar.gz && tar xzf autoconf-2.67.tar.gz && cd autoconf-2.67 && ./configure && make install && cd .. && rm -rf autoconf-2.67 && rm autoconf-2.67.tar.gz

# setup workspace
RUN rm -rf /tmp/workspace
RUN mkdir -p /tmp/workspace

# output dir is mounted

ADD build.rb /tmp/build.rb
CMD ["ruby", "/tmp/build.rb", "/tmp/workspace", "/tmp/output", "/tmp/cache"]
