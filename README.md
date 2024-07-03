# Ruby + JRuby Builder for Heroku

This repo contains scripts to build binaries locally and on GitHub Actions.

## Building with GitHub actions

Navigate to GithHub actions. Select the workflow:

- [Build Ruby](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/build_ruby.yml)
- [Build JRuby](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/build_jruby.yml)

Then click the drop down "Run workflow" and enter the desired Ruby version.

Employees of Heroku see: [The Ruby language guides](https://github.com/heroku/languages-team/tree/main/languages/ruby) (not public) for additional details on building and deploying Ruby versions.

## Install

- Download the repo
- Install [Rust](https://www.rust-lang.org/tools/install).
- Install [Docker](https://www.docker.io/gettingstarted/).

## Run it locally

List available rust scripts:

```
$ cargo run --bin
# ...
Available binaries:
    jruby_build
    jruby_changelog
    jruby_check
    ruby_build
    ruby_changelog
    ruby_check
```

Binaries are prefixed with either `ruby` or `jruby`.

To see the arguments required to a binary, call it without args or with `-- --help`:

```
$ cargo run --bin ruby_build
$ cargo run --bin ruby_build -- --help
  # ...
Usage: ruby_build --arch <ARCH> --version <VERSION> --base-image <BASE_IMAGE>

Options:
      --arch <ARCH>
      --version <VERSION>
      --base-image <BASE_IMAGE>
  -h, --help                     Print help
```

To pass arguments into a binary you have to use a double dash (`--`) separator (to let cargo know you're not trying to give it an argument).

For example:

```
$ cargo run --bin ruby_check -- --version 3.1.6 --arch arm64 --base-image heroku-24
# ...
- Done (finished in 4.9s)

## Ruby 3.1.6 linux/arm64 for heroku-24

- Rubygems version: 3.3.27
- Ruby version: ruby 3.1.6p260 (2024-05-29 revision a777087be6) [aarch64-linux]
```

Two directories are manipulated when running scripts `cache` and `ouput`. Downloaded files will live in `cache` and built/packaged files live in the `output` directory.

## Development

For more details see `.github/workflows/ci.yml`.

Run unit tests:

```
$ cargo test
```
