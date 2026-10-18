# Releasing

Tag-driven, from `main`.

## Steps

1. Run `npm run bump:version -- <version>` on a release-preparation branch. The bump updates Cargo,
   the npm workspaces and lockfile, the documented Action pin, and the Release workflow's default
   version. `npm run check:versions` asserts those sites agree before a release can proceed.
2. Review and merge that change.
3. Create `v<version>` at the merged commit.
4. Run the `Release` workflow manually against that exact tag.

## The Release workflow

Its default `publish: false` rehearses all platform builds and npm packages without creating a
release. `publish: true` creates the GitHub Release with checksums, Sigstore signature bundles and
build provenance, then publishes to each enabled registry.

- The `github-release`, `crates-io` and `npm` GitHub environments should allow only `v*` tags and
  have no required reviewers, as in CukeDedup.
- Repository write access and the workflow's exact-tag, merged-`main` ancestry, and version checks
  control dispatch; there is no owner-only or manual reviewer gate.
- Set `PUBLISH_NPM=true` and, after crates.io setup, `PUBLISH_CRATES=true` as repository variables
  to enable those jobs.

### If npm publication stops partway through

Dispatch the workflow again on the same tag with `publish: true` and `publish_npm_only: true`. This
verifies the existing GitHub Release and its live tag at the dispatched commit, rebuilds and checks
the npm binaries against its archived binaries, then publishes only missing package versions. It
does not recreate the GitHub Release or publish the crate.

## Checking a release

Check the individual GitHub Release, npm, and crate jobs and their registry pages before calling the
release complete; a skipped registry job did not publish anything. The npm website may cache an
older version even when the registry's `latest` tag has advanced.

## The first crate version

The first crate version must be published manually by a crates.io owner before trusted publishing
can be configured. For v0.2.0, use a clean checkout of the existing, immutable tag, authenticate
with your own crates.io token using `cargo login`, and run:

```sh
git switch --detach v0.2.0
cargo publish --dry-run --locked
cargo publish --locked
```

The dry run has already passed on the v0.2.0 tag. After crates.io lists that version, configure a
trusted publisher for `figueiredoluiz/vitest-dedup`, workflow `release.yml`, and GitHub environment
`crates-io`. Create that tag-restricted GitHub environment, then enable `PUBLISH_CRATES=true`.

Later release dispatches use a short-lived crates.io credential from OIDC; no registry token belongs
in GitHub secrets. The already-published v0.2.0 tag cannot gain this new workflow; the automated
crate job applies to later tags.

Back to the [documentation index](README.md).
