# Ruby Builder for Heroku
This uses [Docker](http://docker.io) to build MRI ruby binaries locally for the [heroku ruby buildpack](https://github.com/heroku/heroku-buildpack-ruby).

## Building a Ruby

### Assumptions
I'm assuming you've [already setup Docker](https://www.docker.io/gettingstarted/).

The directory layout used by this script inside the docker container is as follows:

* It builds the binary as tarball and puts it in `$OUTPUT_DIR`.
* All the work is performed in a clean room `/tmp/workspace`.
* The artifacts downloaded for the build are put into `/tmp/cache` or specified by `$CACHE_DIR`.
* Finally, the `/app` directory is there like in a normal cedarish app, so we'll prefix things within this directory so the load paths are useable and fast lookups when using the `--enable-load-relative` flag. We'll need special build rubies for ruby 1.9.2/1.8.7 since `--enable-load-relative` is broken there.

#### Stacks
This build tool supports heroku's multiple stacks. The built rubies will go in the `builds/` directory.

### CircleCI Building
The binaries are [can be built on CircleCI](https://circleci.com/workflow-run/7a131583-15ba-4247-a10f-50dd7a7082a6) using [Workflows](https://circleci.com/docs/2.0/workflows/). There are 4 jobs that are linked into a single workflow:

* Build a Docker Image using the Heroku Stack Image
* Build Ruby
* Test in the Ruby Buildpack
* Upload to S3

There is a workflow for each Heroku Stack, so when a project is kicked off all the stacks can be built in parallel.

To kick off the project, two API calls need to be made:

* Set the Ruby Version
```sh
curl -X POST --header "Content-Type: application/json" -d '{"name":"RUBY_VERSION", "value":"<ruby-version>}' https://circleci.com/api/v1.1/project/github/hone/docker-heroku-ruby-builder/envvar?circle-token=<circle token>
```
* Kick off the Job
```sh
curl -X POST --header "Content-Type: application/json" -d '{"branch": "automation"}' "https://circleci.com/api/v1.1/project/github/hone/docker-heroku-ruby-builder/build?circle-token=<circle token>"
```

#### Environment Variables
These enivronment variables are used by the CircleCI project:

* CIRCLE_TOKEN
* DOCKER_PASSWORD
* DOCKER_USERNAME
* HEROKU_API_KEY
* RUBY_VERSION
* STAGING_AWS_ACCESS_KEY_ID
* STAGING_AWS_SECRET_ACCESS_KEY
* STAGING_BUCKET_NAME

### Local Building

CircleCI [builds and publishes](https://circleci.com/gh/hone/docker-heroku-ruby-builder/226) images to [Docker Hub](https://hub.docker.com/r/hone/ruby-builder) that are used for building rubies. You can just pull the images directly or if you want to build your own you can use the `Makefile` tasks.

```sh
make docker-image STACK=cedar-14
```

```sh
bundle exec rake new[2.6.0,heroku-18] && \
bash rubies/heroku-18/ruby-2.6.0.sh && \
bundle exec rake upload[2.6.0,heroku-18] && \
bundle exec rake test[2.6.0,heroku-18] && \
\
bundle exec rake new[2.6.0,heroku-16] && \
bash rubies/heroku-16/ruby-2.6.0.sh && \
bundle exec rake upload[2.6.0,heroku-16] && \
bundle exec rake test[2.6.0,heroku-16] && \
\
bundle exec rake new[2.6.0,cedar-14] && \
bash rubies/cedar-14/ruby-2.6.0.sh && \
bundle exec rake upload[2.6.0,cedar-14] && \
bundle exec rake test[2.6.0,cedar-14] && \
\
echo "Done building 2.6.0 for cedar-14, heroku-16, and heroku-18"
```

When it's complete you should now see `builds/<stack>/ruby-<ruby-version>.tgz`.

If you set the env vars `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, you can upload them to s3. By default, we upload to the `heroku-buildpack-ruby` s3 bucket.

```sh
$ bundle exec rake upload[2.2.2,cedar-14]
```

#### Docker Enviroment Variables

To configure the build, we use environment variables. All of them are listed below:

* `VERSION` - This is the ruby version being used. It's expected to be in the format: `"#{MAJOR}.#{MINOR}.#{TEENY}-p#{PATCH}"`. This option is required.
* `BUILD` - If this flag is set (can be set to anything for true), then hammer-ruby will build a "build" ruby. This sets the prefix to `"/tmp/ruby-#{MAJOR}.#{MINOR}.#{TEENY}"`. This is required for ruby `1.8.7` and `1.9.2` since the `--enable-load-relative` flag does not work properly. `--enable-load-relative` allows a ruby to be executed from anywhere and not just the prefix directory. By required, you need to build two binaries: one where `BUILD` is false (runtime ruby) and where `BUILD` is true (build ruby).
* `DEBUG` - If this flag is set (can be set to anything for true), then hammer-ruby will set debug flags in the binary ruby being built.
* `RUBYGEMS_VERSION` - This allows one to specify the Rubygems version being used. This is only required for Ruby 1.8.7 since it doesn't bundle Rubygems with it.
* `GIT_URL` - If this option is used, it will override fetching a source tarball from <http://ftp.ruby-lang.org/pub/ruby> with a git repo. This allows building ruby forks or trunk. This option also supports passing a treeish git object in the URL with the `#` character. For instance, `git://github.com/hone/ruby.git#ruby_1_8_7`.
* `S3_BUCKET_NAME` - This option is the S3 bucket name containing of dependencies for building ruby. If this option is not specified, hammer-ruby defaults to "heroku-buildpack-ruby". The dependencies needed are `libyaml-0.1.4.tgz` and `libffi-3.0.10.tgz`.
* `JOBS` - the number of jobs to run in parallel when running make: `make -j<jobs>`. By default this is 2.
