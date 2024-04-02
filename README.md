# Ruby Builder for Heroku

This repo contains scripts to build binaries locally and on GitHub Actions.

## Building with GitHub actions

Navigate to GithHub actions. Select the workflow "Build and upload Ruby runtime" then click the drop down "Run workflow" and enter the desired Ruby version. This will trigger a build for all supported stacks. If a version is not supported on a specific stack, add that logic to the `build.rb` file and to the GitHub action yaml logic.

Employees of Heroku see: [The Ruby language guides](https://github.com/heroku/languages-team/tree/main/languages/ruby) (not public) for additional details on building and deploying Ruby versions.

## How it works

Logic lives in the `build.rb` script at the root of this project. It will call `./configure` and `make` with the corresponding inputs. This file is copied into a docker image when `$ bin/build_ruby` is called. See `dockerfiles/Dockerfile.heroku-24` for an example.

Once built it can be invoked with different inputs like:

```
$ export OUTPUT_DIR="./builds"
$ export CACHE_DIR="./cache"
$ docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=3.2.2  -e STACK=heroku-22 hone/ruby-builder:heroku-22
```

This generates a file in the `builds` directory with the given binary. From there it can be uploaded to S3 so customers of the `heroku/ruby` buildpack can download the pre-built binary and put it on the PATH.

## Building Locally

### Assumptions

I'm assuming you've [already setup Docker](https://www.docker.io/gettingstarted/).

The directory layout used by this script inside the docker container is as follows:

* It builds the binary as tarball and puts it in `/tmp/output`.
* All the work is performed in a clean room `/tmp/workspace`.
* The artifacts downloaded for the build are put into `/tmp/cache`.
* Finally, the `/app` directory is there like in a normal cedarish app, so we'll prefix things within this directory so the load paths are useable and fast lookups when using the `--enable-load-relative` flag. We'll need special build rubies for ruby 1.9.2/1.8.7 since `--enable-load-relative` is broken there.

### Stacks

This build tool supports heroku's multiple stacks and two architectures: AMD (x86) and ARM (M1/Gravitron/etc.). The built rubies will go in the `builds/` directory. We also have a `rubies/` directory for ensuring consistent builds. In each of these directories, they're split into a stack folder. All of the heroku-22 builds will be in `builds/heroku-22/` for instance.

### Building

For an example of the building flow see `.github/workflows`.  Example:

```
$ bin/activate_docker heroku-24
$ bin/build_ruby heroku-24 3.2.3
```

### Building a GIT_URL release

Prior versions of this codebase supported building a ruby binary from git url. This functionality has been removed. You can view git history for inspiration for how you might want to add it in the future.

### Docker Enviroment Variables

To configure the build, we use environment variables. All of them are listed below:

* `VERSION` - This is the ruby version being used. It's expected to be in the format: `"#{MAJOR}.#{MINOR}.#{TEENY}-p#{PATCH}"`. This option is required.
* `BUILD` - If this flag is set (can be set to anything for true), then hammer-ruby will build a "build" ruby. This sets the prefix to `"/tmp/ruby-#{MAJOR}.#{MINOR}.#{TEENY}"`. This is required for ruby `1.8.7` and `1.9.2` since the `--enable-load-relative` flag does not work properly. `--enable-load-relative` allows a ruby to be executed from anywhere and not just the prefix directory. By required, you need to build two binaries: one where `BUILD` is false (runtime ruby) and where `BUILD` is true (build ruby).
* `DEBUG` - If this flag is set (can be set to anything for true), then hammer-ruby will set debug flags in the binary ruby being built.
* `RUBYGEMS_VERSION` - This allows one to specify the Rubygems version being used. This is only required for Ruby 1.8.7 since it doesn't bundle Rubygems with it.
* `GIT_URL` - If this option is used, it will override fetching a source tarball from <http://ftp.ruby-lang.org/pub/ruby> with a git repo. This allows building ruby forks or trunk. This option also supports passing a treeish git object in the URL with the `#` character. For instance, `git://github.com/hone/ruby.git#ruby_1_8_7`.
* `S3_BUCKET_NAME` - This option is the S3 bucket name containing of dependencies for building ruby. If this option is not specified, hammer-ruby defaults to "heroku-buildpack-ruby". The dependencies needed are `libyaml-0.1.4.tgz` and `libffi-3.0.10.tgz`.
* `JOBS` - the number of jobs to run in parallel when running make: `make -j<jobs>`. By default this is 2.

## Development

Run unit tests

```
$ bundle exec rspec
```

Build a binary using current code and run it

```
$ bundle exec rake "generate_image[heroku-22]" && bash rubies/heroku-22/ruby-3.1.2.sh
$ docker run -v $(PWD)/builds/heroku-22:/tmp/output hone/ruby-builder:heroku-22 bash -c "mkdir /tmp/unzipped && tar xzf /tmp/output/ruby-3.1.2.tgz -C /tmp/unzipped && echo 'Rubygems version is: ' &&  /tmp/unzipped/bin/gem -v"
```

