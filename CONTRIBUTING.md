# Contributing to Frust

Thanks for helping make Frust better. This page covers where things go, how to check your work,
and what a pull request should look like.

## Where things go

- Questions and ideas: [GitHub Discussions](https://github.com/frust-rs/frust/discussions).
- Bugs and feature requests: the [issue forms](https://github.com/frust-rs/frust/issues/new/choose).
- Security vulnerabilities: follow [SECURITY.md](SECURITY.md). Never open a public issue for one.

## Setup and checks

Build, run, and test instructions live in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). The verify
gate for the root workspace is:

```
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

The standalone example workspaces (for instance `examples/huddle`) are excluded from the root
graph and have their own gates, run from their own directories; docs/DEVELOPMENT.md lists them.

Run `scripts/install-hooks.sh` once after cloning to install the repository's git hooks.

## Branching

`main` is the only long-lived branch and must always build and pass the gate. Fork the repository,
branch from `main`, and open a pull request into `main`. There are no direct pushes. Outside pull
requests are squash-merged; maintainers may land a multi-commit feature branch with a merge commit.

## Releases and versions

See [docs/RELEASING.md](docs/RELEASING.md) for the release process.

- All crates share one version.
- Before 1.0, a minor version (0.x) may break API and a patch release may not.
- Releases are tags `vX.Y.Z` on `main`. A `release/X.Y` branch exists only when a patch release is
  needed after `main` has moved on; fixes land on `main` first and are cherry-picked (label
  `backport`).
- Pre-releases are published as `X.Y.0-rc.N`.
- There are no beta or development branches. Choose stable, a release candidate, or git `main`
  through the Cargo version requirement.
- The minimum supported Rust version is the workspace `rust-version`. Raising it is a
  minor-version change.

## Pull request expectations

- One topic per pull request.
- Tests for behaviour changes.
- Update docs in the same pull request. [docs/DOC_POLICY.md](docs/DOC_POLICY.md) lists the managed
  docs and their size budgets; a new platform gap gets an entry in
  [docs/LIMITATIONS.md](docs/LIMITATIONS.md).
- Dependency versions are pinned deliberately (see the Version-Pin Policy in
  [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)). Do not bump them in a feature pull request.

## Commit messages

Use an imperative subject line. Do not add tool-attribution trailers or links to private chat or
agent sessions; CI rejects them.

## AI-assisted contributions

AI assistance is allowed. You are the author: you must understand and be able to explain every
line you submit, and you say so in the pull request description when assistance was substantial.
Unreviewed generated changes are closed.

## Licensing of contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

No CLA and no sign-off line is required.

## Code of Conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).
