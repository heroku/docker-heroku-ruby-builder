# Ruby Builder for Heroku
This uses [Docker](http://docker.io) to build MRI ruby binaries locally in a cedar image for the [heroku ruby buildpack](https://github.com/heroku/heroku-buildpack-ruby).

## Building a Ruby

### Assumptions
I'm assuming you've [already setup Docker](https://www.docker.io/gettingstarted/).

The directory layout used by this script inside the docker container is as follows:

* It builds the binary as tarball and puts it in `/tmp/output`.
* All the work is performed in a clean room `/tmp/workspace`.
* The artifacts downloaded for the build are put into `/tmp/cache`.
* Finally, the `/app` directory is there like in a normal cedar app, so we'll prefix things within this directory so the load paths are useable and fast lookups when using the `--enable-load-relative` flag.


### Building

Next you'll need to pull in the [docker image](https://index.docker.io/u/hone/ruby-builder) that's setup to build the binaries.

```sh
$ docker.io pull hone/ruby-builder
```

Next we need to create a directory where docker will put the build tarball and store the cached files:

```sh
$ mkdir -p /tmp/output
$ mkdir -p /tmp/cache
```

We can now build the tarball. docker supports mounting a directory that's shared with the local computer by using `-v $LOCAL_DIR:$DOCKER_DIR`. docker also uses `-e` to pass in environment variables to the compile process. This allows us to specify the version of ruby, in this example it'll be `-e VERSION=2.1.1`. You'll run a command like the one below:

```sh
$ docker.io run -v /tmp/output:/tmp/output -v /tmp/cache:/tmp/cache -e VERSION=2.1.2 hone/ruby-builder
```

If everything has been built successful, if we check the local output directory, we should see the tarball.

```sh
$ ls /tmp/output
ruby-2.1.2.tgz
```

To ensure consistent builds, inside the `rubies/` directory will be `.sh` files corresponding to the settings used to build that version of ruby. You can then simply execute the sh file to build that ruby.

```sh
$ sh rubies/ruby-2.1.2.sh
```

### Enviroment Variables
To configure the build, we use environment variables. All of them are listed below:

* `VERSION` - This is the ruby version being used. It's expected to be in the format: `"#{MAJOR}.#{MINOR}.#{TEENY}-p#{PATCH}"`. This option is required.
* `BUILD` - If this flag is set (can be set to anything for true), then hammer-ruby will build a "build" ruby. This sets the prefix to `"/tmp/ruby-#{MAJOR}.#{MINOR}.#{TEENY}"`. This is required for ruby `1.8.7` and `1.9.2` since the `--enable-load-relative` flag does not work properly. `--enable-load-relative` allows a ruby to be executed from anywhere and not just the prefix directory. By required, you need to build two binaries: one where `BUILD` is false (runtime ruby) and where `BUILD` is true (build ruby).
* `DEBUG` - If this flag is set (can be set to anything for true), then hammer-ruby will set debug flags in the binary ruby being built.
* `RUBYGEMS_VERSION` - This allows one to specify the Rubygems version being used. This is only required for Ruby 1.8.7 since it doesn't bundle Rubygems with it.
* `GIT_URL` - If this option is used, it will override fetching a source tarball from <http://ftp.ruby-lang.org/pub/ruby> with a git repo. This allows building ruby forks or trunk. This option also supports passing a treeish git object in the URL with the `#` character. For instance, `git://github.com/hone/ruby.git#ruby_1_8_7`.
* `S3_BUCKET_NAME` - This option is the S3 bucket name containing of dependencies for building ruby. If this option is not specified, hammer-ruby defaults to "heroku-buildpack-ruby". The dependencies needed are `libyaml-0.1.4.tgz` and `libffi-3.0.10.tgz`.
* `JOBS` - the number of jobs to run in parallel when running make: `make -j<jobs>`. By default this is 2.
