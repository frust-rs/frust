# Releasing

The maintainer's procedure for cutting a Frust release. Branching and pull-request rules live in
[CONTRIBUTING.md](../CONTRIBUTING.md); the gates are in [DEVELOPMENT.md](DEVELOPMENT.md) § Test and
run in [ci.yml](../.github/workflows/ci.yml) (jobs `fmt`, `clippy`, `test`, `standalone`, `hygiene`,
aggregate `ci-ok`).

## Versioning

- One version for every crate: `[workspace.package] version` in the root `Cargo.toml`.
- Before 1.0, a minor version (0.x) may break API; a patch release may not.
- The minimum supported Rust version is the workspace `rust-version`; raising it is a minor-version
  change. The pinned development toolchain in `rust-toolchain.toml` is a separate, newer setting.
- Version pins on third-party dependencies are governed by
  [DEVELOPMENT.md](DEVELOPMENT.md) § Version-Pin Policy and do not change as part of a release.

### What a bump touches

1. `version` under `[workspace.package]`.
2. The `version = "…"` on every internal row of `[workspace.dependencies]` (the rows under the
   `# intra-workspace` comment) and on every literal path dependency in a crate manifest, for example
   `frust-drive = { path = "../frust-drive", version = "0.5.0" }` in `crates/frust-cli`. Find them
   with `git grep -n 'path = ".*version = "'`.
3. `plugins/clean-signals-frust` is a standalone workspace with its own literal package `version`;
   raise it too.
4. The `Cargo.lock` of each standalone workspace records the frust crate versions. Refresh each by
   running `cargo metadata --format-version 1 > /dev/null` inside its directory (the list is in
   [DEVELOPMENT.md](DEVELOPMENT.md) § Test), then commit the lock files.
5. The root `Cargo.lock`, refreshed by any `cargo build --workspace`.

Members with `publish = false` (`frust-testing`, the demo and gallery packages) are never published.

## Branches and tags

- `main` is the only long-lived branch. A release is the tag `vX.Y.Z` on a commit of `main`.
- A `release/X.Y` branch is cut from the tag only when a patch is needed after `main` has moved on.
  Fix on `main` first, then cherry-pick onto the release branch for pull requests labelled
  `backport`. Tag the patch (`vX.Y.Z`) on the release branch.
- Pre-releases are tagged and published as `X.Y.0-rc.N`.
- There are no beta or development branches.

## Release checklist

1. The gate is green on `main` (the `ci-ok` check of the latest `main` commit).
2. Licence texts are in place in every publishable package:
   `scripts/sync-licenses.sh --check`. If it names missing or differing copies, run
   `scripts/sync-licenses.sh` (optionally `--only <dir>…`) and commit the result. See
   [sync-licenses.sh](../scripts/sync-licenses.sh), [LICENSE-MIT](../LICENSE-MIT) and
   [LICENSE-APACHE](../LICENSE-APACHE).
3. Land a version-bump commit as described above, through a pull request into `main`.
4. Check that everything packages and verifies, without uploading:
   `cargo publish --workspace --dry-run --locked`.
5. Tag the bump commit: `git tag -a vX.Y.Z -m "Frust X.Y.Z"`, then `git push origin vX.Y.Z`.
6. Publish (next section).
7. Create the GitHub release from the tag, with notes for the changes since the previous tag.

## Publishing

```bash
cargo publish --workspace --locked
```

Cargo 1.90 or newer publishes the workspace members in dependency order and waits for each to
appear in the index before publishing its dependents. Members with `publish = false` are skipped;
`--exclude <SPEC>` leaves a further package out. `cargo --version` should show 1.90 or newer.

`scripts/ci/publish-remaining.sh` wraps this for a release that stops part-way (a rate limit, a
lost token): with no flags it prints every publishable package whose version the crates.io index
does not list yet; `--publish` runs `cargo publish --locked` for exactly those packages, and
exits 1 with the remainder and cargo's retry time when the registry refuses, so re-running it
resumes. `--no-verify` and `--dry-run` pass through to cargo.

### Rate limits

crates.io limits publishing (read from crates.io's published source on 2026-10-02; re-check before
each release, since the limits can change):

- New crates: a burst of 5, then one per 10 minutes.
- New versions of existing crates: a burst of 30, then one per minute.
- The limits can be raised by writing to help@crates.io.

A first release of the whole workspace creates every crate and therefore hits the new-crate limit.
A version that is on crates.io cannot be uploaded again, so after a stop wait the stated time and
publish only what is still missing: re-run with `--exclude <crate>` for each crate already
published. Do not change the version to get around a stop.

### First release and Trusted Publishing

The first release of a crate is published from a maintainer's machine with an API token
(`cargo login`), because crates.io can only configure Trusted Publishing for a crate that already
exists. After a crate exists, its settings on crates.io name this repository, the workflow
`release.yml` and the environment `release`; from then on pushing a tag `vX.Y.Z` runs
`.github/workflows/release.yml`: `verify` checks that the tag names the workspace version, that
the licence copies are in sync and that every package builds as published; `publish` exchanges
the GitHub OIDC identity for a short-lived token (`rust-lang/crates-io-auth-action`, 30 minutes)
and runs `scripts/ci/publish-remaining.sh --publish --no-verify`, so a run that stops on the rate
limit is re-run and resumes; `github-release` then creates the release with generated notes.

### Ownership

Crate ownership is held by the `frust-rs` GitHub organisation's `owners` team. New crates are
given to that team after their first publish (`cargo owner --add github:frust-rs:owners <crate>`),
so that no single person is the sole owner.

## Yanking

Yank only a release that is broken (does not build, corrupts data, or has a security defect):
`cargo yank --version X.Y.Z <crate>`. Never yank to hide or retract a release that merely turned
out to be undesirable; fix forward with a patch. Yanking does not delete a version, and a version
number can never be reused. A yanked release gets a note in its GitHub release explaining why.

## Standalone-workspace gates

The root gate does not cover the standalone workspaces (`examples/huddle`, `examples/playground`,
`examples/native-widgets-demo`, `examples/design-system-sample`, `examples/material3-demo`,
`plugins/clean-signals-frust`, and the other excluded examples). Their gates must pass, run from
their own directories, before tagging; [DEVELOPMENT.md](DEVELOPMENT.md) § Test is canonical and the
`standalone` job of [ci.yml](../.github/workflows/ci.yml) runs the same set. Refresh their lock
files as described under "What a bump touches" first, so they are tested against the bumped
versions.
