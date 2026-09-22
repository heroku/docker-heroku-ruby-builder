# Ruby + JRuby Builder for Heroku

This repo contains scripts to build binaries locally and on GitHub Actions.

## Building with GitHub actions

Navigate to GithHub actions. Select the workflow:

- [Build Ruby](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/build_ruby.yml)
- [Build JRuby](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/build_jruby.yml)

Then click the drop down "Run workflow" and enter the desired Ruby version.

Employees of Heroku see: [The Ruby language guides](https://github.com/heroku/languages-team/tree/main/languages/ruby) (not public) for additional details on building and deploying Ruby versions.

## Creating a Dev Center changelog

A [Dev Center changelog](https://devcenter.heroku.com/changelog) entry for a newly built version can be created via the Dev Center private API:

- The [Build Ruby](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/build_ruby.yml) and [Build JRuby](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/build_jruby.yml) workflows can create the entry automatically after all builds for the version succeed, but only when the **"Publish the Dev Center changelog after builds"** option is enabled. It defaults to off, so by default no entry is created; when enabled, the entry is **published live** immediately (these workflows have no draft option).
- The [Create Dev Center changelog](https://github.com/heroku/docker-heroku-ruby-builder/actions/workflows/create_changelog.yml) workflow creates a single entry on demand. Pick the engine and version, and optionally publish (default draft).

Both require a repository secret **`HEROKU_DEVCENTER_API_TOKEN`**: a Heroku OAuth token for an **active admin** Dev Center user (an `api_client` token is rejected). It is used as the HTTP Basic auth password (with an empty username) against `POST /api/v1/private/changelog_items`.

To preview the generated markdown locally without contacting Dev Center:

```
$ cargo run --bin ruby_changelog -- print --version 3.4.1
```

To create the entry through the Dev Center API, choose whether to publish it or leave it as an unpublished draft with the required `--status` flag (this contacts Dev Center and requires `HEROKU_DEVCENTER_API_TOKEN`):

```
$ cargo run --bin ruby_changelog -- devcenter --version 3.4.1 --status draft
$ cargo run --bin ruby_changelog -- devcenter --version 3.4.1 --status published
```

Publishing is guarded against duplicates: `--status published` first scans entries from the last 7 days and, if one already has the same title and published state, reports it and exits non-zero instead of creating a duplicate (content is intentionally not compared, so a regenerated body cannot slip a second entry past the guard). Drafts are never deduplicated, so `--status draft` always creates an entry (a convenient check that the API and token work).

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
