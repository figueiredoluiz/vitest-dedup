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

### The recall corpus comparison

Every rehearsal and publish first runs the `compare` job. It finds the newest published release
other than the one being built, downloads that release's Linux archive, checks its sha256 and its
build-provenance attestation, and runs the recall corpus (`fixtures/recall`) through the new build
and the previous binary with `scripts/check/evaluate-corpus.mjs`. The outcome classes are described
in [the recall corpus README](../fixtures/recall/README.md#comparing-two-binaries).

- A rehearsal prints the section that the notes would carry in the job summary and in the
  `corpus-comparison` artifact, which also holds the full per-case detail.
- A publish adds that section to the top of the generated release notes, between markers, after the
  release is created. A retry replaces the section rather than adding another.
- A comparison that cannot be made (the previous archive missing, altered or unattested, or the
  evaluation failing) fails the job and so stops the release. It is never written as a clean result.
  The first release of a repository has no earlier release and says so in its notes.
- Regressions are reported, not blocking: the section lists them, and whether to ship is the
  releaser's decision.
- An npm-only recovery does not run the comparison and does not touch the release notes.
- If the step that writes the section fails after the release is created, the `publish` job fails and
  the crate, npm and site jobs do not run. Re-run the `publish` job: it updates the existing release
  and puts the section back, once. Do not use an npm-only recovery for this, which leaves the notes
  alone.

To run it locally against a published release: `GH_REPO=<owner>/<repo> node
scripts/release/release-comparison.mjs compare --version <version> --binary <binary> --out <dir>
--target <platform key>`, with a `--target` that matches the machine, such as `darwin-arm64`.

### If npm publication stops partway through

Dispatch the workflow again on the same tag with `publish: true` and `publish_npm_only: true`. This
verifies the existing GitHub Release and its live tag at the dispatched commit, rebuilds and checks
the npm binaries against its archived binaries, then publishes only missing package versions. It
does not recreate the GitHub Release or publish the crate.

## Website rebuild

After a publish, the `notify-site` job asks the website repository (`vitest-dedup-site`) to rebuild by
sending it a `release-published` `repository_dispatch` event.

- It waits for the GitHub release and the registry jobs, and does not run after a failed crate or npm
  job, so the site never leads what users can install. A registry job that is switched off (a
  `PUBLISH_NPM` or `PUBLISH_CRATES` variable that is not `true`) is skipped, not failed, so the site
  is still rebuilt after the GitHub release.
- It runs for a normal publish, and for an npm-only recovery once the packages land. It never runs for a
  rehearsal.
- A failure here unpublishes nothing. Re-run only that job.
- It needs the repository secret `SITE_DISPATCH_TOKEN`: a fine-grained personal access token limited to
  the website repository, with `Contents: read and write`, which `repository_dispatch` requires. This is
  the one long-lived secret in the workflow, because a cross-repository dispatch cannot use OIDC. Without
  it the job fails with a message saying so.
- The website repository must have a workflow that runs `on: repository_dispatch` with the type
  `release-published`.
- Prereleases trigger it too; the dispatch carries no version.

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
