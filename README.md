# Ruby + JRuby Builder for Heroku

This repo contains scripts to build binaries locally and on GitHub Actions.

## Building with GitHub actions

Navigate to GithHub actions. Select the workflow "Build and upload (j?)Ruby runtime" then click the drop down "Run workflow" and enter the desired Ruby version. This will trigger a build for all supported stacks. If a version is not supported on a specific stack, add that logic to the `inside_docker/*.rs` file and to the GitHub action yaml logic.

Employees of Heroku see: [The Ruby language guides](https://github.com/heroku/languages-team/tree/main/languages/ruby) (not public) for additional details on building and deploying Ruby versions.

## Install

- Download the repo
- Install [Rust](https://www.rust-lang.org/tools/install).
- Install [Docker](https://www.docker.io/gettingstarted/).

## Run it locally

All of the logic of this repo lives in rust scripts. List available rust scripts:

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

To see the arguments required to a binary, call it without args:

```
$ cargo run --release --bin ruby_build
  --arch <ARCH>
  --version <VERSION>
  --base-image <BASE_IMAGE>
```

To pass arguments into a binary you have to use a `--` separator (to let cargo know you're not trying to give it an argument). For example:

```
$ cargo run --release --bin ruby_check -- --version 3.1.6 --arch arm64 --base-image heroku-24
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

## Why Rust?

I bet you're thinking "Why not something simpler like bash or Ruby?" This library was originally written in Ruby and shelled out. That caused bootstrapping problems, for example when rolling out ARM support, the github action for installing Ruby did not yet support ARM so we had to re-write the logic in Bash (or bootstrap our own version of Ruby with bash). We chose to re-write the library in bash. So why not keep it in bash? Even though bash doesn't need "bootstrapping" authors rely on system tools and packages which may or may not already be installed, and may or may not vary between operating systems. For example GNU grep uses different arguments than BSD (mac) grep. So while there's not a "bash bootstrapping problem" installing dependencies and ensuring scripts work across multiple platforms is tricky. It's easy to write quick scripts, but hard to maintain and do well.

Don't you have a Rust bootstrapping problem now? As of Ruby 3.2 YJIT requires a Rust toolchain to support the `--enable-yjit` config flag, so Rust is already a requirement of a full Ruby install. That means that even if we didn't write our scripts in Rust, we still need to have it available on the system when we build Ruby anyway. It's also historically been an easy to install language.
