# Ruby Builder for Heroku
This uses [Docker](http://docker.io) to build MRI ruby binaries locally in a cedar image for the [heroku ruby buildpack](https://github.com/heroku/heroku-buildpack-ruby).

## Building a Ruby

### Building Locally

#### Assumptions
I'm assuming you've [already setup Docker](https://www.docker.io/gettingstarted/).

The directory layout used by this script inside the docker container is as follows:

* It builds the binary as tarball and puts it in `/tmp/output`.
* All the work is performed in a clean room `/tmp/workspace`.
* The artifacts downloaded for the build are put into `/tmp/cache`.
* Finally, the `/app` directory is there like in a normal cedarish app, so we'll prefix things within this directory so the load paths are useable and fast lookups when using the `--enable-load-relative` flag. We'll need special build rubies for ruby 1.9.2/1.8.7 since `--enable-load-relative` is broken there.

#### Stacks

This build tool supports heroku's multiple stacks. The built rubies will go in the `builds/` directory. We also have a `rubies/` directory for ensuring consistent builds. In each of these directories, they're split into a stack folder. All of the cedar-14 builds will be in `builds/cedar-14/` for instance.

#### Building

First we'll need to generate the docker images needed for building the appropriate stack.

```sh
$ bundle exec rake "generate_image[cedar-14]"
```

Generate a ruby build script:

```sh
$ bundle exec rake new[2.2.2,cedar-14]
```

From here, we can now execute a ruby build:

```
$ bash rubies/cedar-14/ruby-2.2.2.sh
```

When it's complete you should now see `builds/cedar-14/ruby-2.2.2.tgz`.

If you set the env vars `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, you can upload them to s3. By default, we upload to the `heroku-buildpack-ruby` s3 bucket.

```sh
$ bundle exec rake upload[2.2.2,cedar-14]
```

#### Support latest stacks (and 2.7.x for cedar-14)

When a new Ruby version releases you will want to build support for all stacks.

```sh
bundle exec rake "new[3.1.2,heroku-20]" &&
bundle exec rake "new[3.1.2,heroku-22]" &&
bash rubies/heroku-20/ruby-3.1.2.sh &&
bash rubies/heroku-22/ruby-3.1.2.sh &&
echo "Done building"
say "Done building"


bundle exec rake "upload[3.1.2,heroku-20]" &&
bundle exec rake "upload[3.1.2,heroku-22]" &&
bundle exec rake "test[3.1.2,heroku-20]" &&
bundle exec rake "test[3.1.2,heroku-22]" &&
echo "Done uploading"
say "Done uploading"
```

#### Building a GIT_URL release

Sometimes a version might need to be tested, for example a commit on Ruby trunk.

If you're building from a specific commit, then fork `ruby/ruby` to your own repo.

To build it first generate a new file:

```
bundle exec rake new[2.6.0,heroku-18]
```

Then add in the destination to the GIT_URL:

```
docker run -v $OUTPUT_DIR:/tmp/output -v $CACHE_DIR:/tmp/cache -e VERSION=2.6.0 -e GIT_URL=https://github.com/schneems/ruby#schneems/bundler -e STACK=heroku-18 hone/ruby-builder:heroku-18
```

If you need to use a branch you can put it in the url after the `#`.

#### Docker Enviroment Variables

To configure the build, we use environment variables. All of them are listed below:

* `VERSION` - This is the ruby version being used. It's expected to be in the format: `"#{MAJOR}.#{MINOR}.#{TEENY}-p#{PATCH}"`. This option is required.
* `BUILD` - If this flag is set (can be set to anything for true), then hammer-ruby will build a "build" ruby. This sets the prefix to `"/tmp/ruby-#{MAJOR}.#{MINOR}.#{TEENY}"`. This is required for ruby `1.8.7` and `1.9.2` since the `--enable-load-relative` flag does not work properly. `--enable-load-relative` allows a ruby to be executed from anywhere and not just the prefix directory. By required, you need to build two binaries: one where `BUILD` is false (runtime ruby) and where `BUILD` is true (build ruby).
* `DEBUG` - If this flag is set (can be set to anything for true), then hammer-ruby will set debug flags in the binary ruby being built.
* `RUBYGEMS_VERSION` - This allows one to specify the Rubygems version being used. This is only required for Ruby 1.8.7 since it doesn't bundle Rubygems with it.
* `GIT_URL` - If this option is used, it will override fetching a source tarball from <http://ftp.ruby-lang.org/pub/ruby> with a git repo. This allows building ruby forks or trunk. This option also supports passing a treeish git object in the URL with the `#` character. For instance, `git://github.com/hone/ruby.git#ruby_1_8_7`.
* `S3_BUCKET_NAME` - This option is the S3 bucket name containing of dependencies for building ruby. If this option is not specified, hammer-ruby defaults to "heroku-buildpack-ruby". The dependencies needed are `libyaml-0.1.4.tgz` and `libffi-3.0.10.tgz`.
* `JOBS` - the number of jobs to run in parallel when running make: `make -j<jobs>`. By default this is 2.


#### How it works

There is a script `build.rb` that was coppied over when the docker container was built. This can be seen in the various `Dockerfile.*` files.
