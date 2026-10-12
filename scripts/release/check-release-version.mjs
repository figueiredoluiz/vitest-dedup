#!/usr/bin/env node
// Asserts every version site in the repository agrees with Cargo.toml.
//
// Cargo.toml is the source of truth because `.github/workflows/release.yml` already refuses to
// build a tag that disagrees with it. This gate extends that one comparison to the rest of the
// tree: the npm launcher's version and its eight `optionalDependencies`, the lockfile, the
// and the documented Action pins. Every one of those can drift independently, and
// each drift ships a different wrong thing -- a launcher that resolves platform packages nobody
// published, a README telling users to pin a version that does not exist, a release with no notes.
//
// Optional environment, used by the release workflow:
//   EXPECTED_VERSION  the version being released; must equal Cargo.toml's.
//   RELEASE_TAG       the tag being released; must be `v<version>`.
import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { TARGETS } from '../../npm/lib/targets.mjs'
import { platformManifest } from './platform-package.mjs'
import { cargoLockVersion, cargoVersion, readmeVersionSites } from './version-sites.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const read = (relative) => readFileSync(join(root, relative), 'utf8')

const version = cargoVersion(read('Cargo.toml'))
let checks = 0
const checked = (message, assertion) => {
  checks += 1
  assertion(message)
}

checked('Cargo.lock disagrees with Cargo.toml', (message) =>
  assert.equal(cargoLockVersion(read('Cargo.lock')), version, message))

const manifest = JSON.parse(read('package.json'))
checked('package.json version differs from Cargo.toml', (message) =>
  assert.equal(manifest.version, version, message))

// The launcher's optional dependencies ARE the platform matrix. A name here that no release job
// builds is a package that will never exist; a target with no entry here is a platform the
// launcher can resolve a package name for but npm will never install.
checked('package.json optionalDependencies do not match npm/prebuilt-targets.json', (message) =>
  assert.deepEqual(
    Object.keys(manifest.optionalDependencies).sort(),
    Object.values(TARGETS).map((target) => target.packageName).sort(),
    message,
  ))
for (const [name, declared] of Object.entries(manifest.optionalDependencies)) {
  checked(`${name} is pinned to ${declared}, not ${version}`, (message) =>
    assert.equal(declared, version, message))
}

checked('package.json bin does not point at the launcher', (message) =>
  assert.deepEqual(manifest.bin, { 'vitest-dedup': 'npm/bin/vitest-dedup.js' }, message))
for (const entry of ['npm/bin/', 'npm/lib/', 'npm/prebuilt-targets.json']) {
  checked(`package.json files allowlist omits ${entry}, so the published package cannot run`, (m) =>
    assert.ok(manifest.files.includes(entry), m))
}

const readme = readmeVersionSites(read('README.md'))
checked('README.md carries no Action version pin; the pattern that finds them has drifted', (m) =>
  assert.ok(readme.versions.length > 0, m))
for (const pinned of readme.versions) {
  checked(`README.md pins version ${pinned}, not ${version}`, (message) =>
    assert.equal(pinned, version, message))
}
checked('README.md carries no exact Action tag; the pattern that finds it has drifted', (m) =>
  assert.ok(readme.actionTags.length > 0, m))
for (const pinned of readme.actionTags) {
  checked(`README.md pins the Action at @v${pinned}, not @v${version}`, (message) =>
    assert.equal(pinned, version, message))
}


// Generated output, present only after `prepare-npm-packages.mjs` has run. Checking it when it
// exists is what makes this gate usable as the last step before publishing.
for (const target of Object.values(TARGETS)) {
  const path = join('npm', 'platforms', target.packageDirectory, 'package.json')
  if (!existsSync(join(root, path))) continue
  checked(`${path} does not match what platform-package.mjs generates at ${version}`, (message) =>
    assert.deepEqual(JSON.parse(read(path)), platformManifest(target, version), message))
}

if (process.env.EXPECTED_VERSION) {
  checked(`Cargo.toml is ${version} but the release is ${process.env.EXPECTED_VERSION}`, (m) =>
    assert.equal(version, process.env.EXPECTED_VERSION, m))
}
if (process.env.RELEASE_TAG) {
  checked(`release tag ${process.env.RELEASE_TAG} does not name version ${version}`, (m) =>
    assert.equal(process.env.RELEASE_TAG, `v${version}`, m))
}

// A floor, for the same reason every other gate in this repository has one: a refactor that made
// the site patterns stop matching would otherwise leave this script passing while comparing almost
// nothing.
//
// Lowered 20 -> 18 when the manual changelog was removed: release notes are generated from commit
// history (`generate_release_notes` in release.yml), so the two checks that asserted a dated
// CHANGELOG section are gone along with the file. The floor tracks the checks that remain, and
// re-measuring it is the only honest way to keep it meaningful.
const MIN_CHECKS = 18
assert.ok(
  checks >= MIN_CHECKS,
  `only performed ${checks} version comparisons, expected at least ${MIN_CHECKS}`,
)
console.log(`ok: ${checks} version sites all agree on ${version}`)
