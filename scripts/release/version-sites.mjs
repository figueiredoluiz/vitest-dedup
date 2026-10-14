// The one place that knows where a version number is written down in this repository.
//
// `bump-version.mjs` rewrites exactly this set and `check-release-version.mjs` verifies exactly
// this set, so the bumper can never rewrite a site the checker does not know about, and the
// checker can never demand a site the bumper cannot reach. Both are pure functions over file text
// here, which is what lets `npm/test/` exercise every pattern without a working tree, a tag, or
// cargo.
//
// `action.yml` deliberately remains a required input without a default; the manual release
// workflow defaults its version to the current Cargo version and must be bumped with it.
// Keep the prerelease suffix within the format accepted by release.yml and the Action installer.
// A second hyphen would pass npm's broader syntax but fail that workflow.
export const SEMVER = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.]+)?$/

// Every multi-line pattern below matches `\r?\n`, never a bare `\n`. Git checks these files out
// with CRLF endings on Windows by default, so a `\n`-anchored pattern silently matches nothing
// there and the bump reports "version not found" on a file that plainly contains one. That is a
// contributor-facing failure, not only a test one: `npm run bump:version` is meant to work on any
// platform the rest of this repository supports.

/** Matches the `version = "X"` line inside Cargo.toml's `[package]` table, and only that one. */
export const CARGO_PACKAGE_VERSION = /(^\[package\]\r?\n(?:.*\r?\n)*?version = ")([^"]+)(")/m

/** The workspace crate's own entry in Cargo.lock. */
export const CARGO_LOCK_VERSION =
  /(^\[\[package\]\]\r?\nname = "vitest-dedup"\r?\nversion = ")([^"]+)(")/m

/** The default version shown by the manual Release workflow dispatch. */
export const RELEASE_WORKFLOW_DEFAULT =
  /(^ {6}version:\r?\n(?: {8}.*\r?\n)*? {8}default: )(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?=\r?$)/m

/** `version: X.Y.Z` inputs in the README's GitHub Action examples. */
export const README_ACTION_VERSION =
  /(^[^\S\r\n]*version: )(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?=\r?$)/gm

/** `figueiredoluiz/vitest-dedup@vX.Y.Z` -- the documented exact Action tag. */
export const ACTION_VERSION_TAG =
  /(figueiredoluiz\/vitest-dedup@v)(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?=\s|$)/g

const one = (text, pattern, label) => {
  const match = text.match(pattern)
  if (!match) {
    throw new Error(`${label} was not found`)
  }
  return match[2]
}

/** Extract the package version from Cargo.toml text; throw when the package version site is absent. */
export function cargoVersion(text) {
  return one(text, CARGO_PACKAGE_VERSION, 'the [package] version in Cargo.toml')
}

/** Extract the vitest-dedup version from Cargo.lock text; throw when its lock entry is absent. */
export function cargoLockVersion(text) {
  return one(text, CARGO_LOCK_VERSION, 'the vitest-dedup entry in Cargo.lock')
}

/** Extract the manual release workflow default version; throw when that version site is absent. */
export function releaseWorkflowVersion(text) {
  return one(text, RELEASE_WORKFLOW_DEFAULT, 'the Release workflow default version')
}

/** Every `version:` pin and exact Action tag the README carries, in document order. */
export function readmeVersionSites(text) {
  return {
    versions: [...text.matchAll(README_ACTION_VERSION)].map((match) => match[2]),
    actionTags: [...text.matchAll(ACTION_VERSION_TAG)].map((match) => match[2]),
  }
}
