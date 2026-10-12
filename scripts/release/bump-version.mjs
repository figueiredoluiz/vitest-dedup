#!/usr/bin/env node
// Single entry point for a version bump: `npm run bump:version -- 0.2.0`.
//
// It rewrites exactly the set of sites `version-sites.mjs` declares, which is exactly the set
// `check-release-version.mjs` verifies -- so a bump performed here always satisfies the release
// gate, and a site one of them learns about is a site both learn about.
//
// The bump is atomic and planned before it is applied: every file is read, every replacement is
// validated (a pattern that matches zero times, or more times than expected, is a hard failure
// rather than a silent partial rewrite), and only then is anything written. A half-bumped tree --
// Cargo.toml at the new version, package.json at the old one -- is worse than no bump, because it
// still builds and still publishes.
//
// Cargo.lock is rewritten directly rather than by invoking `cargo update`. This crate is the only
// workspace member, so its lock entry is a single well-known line; rewriting it keeps the bump
// instant, offline, and free of the serialized-cargo constraint this repository works under.
// cuke-dedup shells out to cargo because it has a second (fuzz) lockfile whose dependency graph
// really does need resolving.
import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  ACTION_VERSION_TAG,
  CARGO_LOCK_VERSION,
  CARGO_PACKAGE_VERSION,
  README_ACTION_VERSION,
  SEMVER,
  cargoVersion,
} from './version-sites.mjs'

export const BUMPED_FILES = ['Cargo.toml', 'Cargo.lock', 'package.json', 'README.md']

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

function replaceOnce(text, pattern, path, replacement) {
  const all = new RegExp(pattern.source, pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`)
  const found = [...text.matchAll(all)].length
  if (found !== 1) {
    throw new Error(`expected exactly one version site in ${path}, found ${found}`)
  }
  return text.replace(pattern, replacement)
}

/**
 * Computes the new contents of every bumped file. Pure: no I/O, so the whole bump is testable
 * against in-memory fixtures rather than against a real checkout.
 *
 * @param {string} version the target version.
 * @param {Record<string, string>} files current contents, keyed by repo-relative path.
 * @returns {{previous: string, planned: Map<string, string>}}
 */
export function planBump(version, files) {
  if (!SEMVER.test(version)) {
    throw new Error(`not a release version: ${version}`)
  }
  for (const path of BUMPED_FILES) {
    if (typeof files[path] !== 'string') {
      throw new Error(`bump needs the contents of ${path}`)
    }
  }

  const previous = cargoVersion(files['Cargo.toml'])
  if (previous === version) {
    throw new Error(`Cargo.toml is already at ${version}`)
  }

  const planned = new Map()
  planned.set(
    'Cargo.toml',
    replaceOnce(files['Cargo.toml'], CARGO_PACKAGE_VERSION, 'Cargo.toml', `$1${version}$3`),
  )
  planned.set(
    'Cargo.lock',
    replaceOnce(files['Cargo.lock'], CARGO_LOCK_VERSION, 'Cargo.lock', `$1${version}$3`),
  )

  // package.json is the one file npm itself rewrites, so a JSON round-trip at two-space indent
  // reproduces its formatting exactly and a structural edit is safe.
  const manifest = JSON.parse(files['package.json'])
  manifest.version = version
  for (const name of Object.keys(manifest.optionalDependencies ?? {})) {
    manifest.optionalDependencies[name] = version
  }
  planned.set('package.json', `${JSON.stringify(manifest, null, 2)}\n`)

  // The README pins the exact release tag, so both its `uses:` ref and `version:` input must
  // move on every bump. A major-only alias is not created by this release process.
  const readmePins = [...files['README.md'].matchAll(README_ACTION_VERSION)].length
  if (readmePins === 0) {
    throw new Error('README.md carries no Action version pin to bump')
  }
  const actionTags = [...files['README.md'].matchAll(ACTION_VERSION_TAG)].length
  if (actionTags === 0) {
    throw new Error('README.md carries no exact Action tag to bump')
  }
  planned.set(
    'README.md',
    files['README.md']
      .replace(README_ACTION_VERSION, `$1${version}`)
      .replace(ACTION_VERSION_TAG, `$1${version}`),
  )

  return { previous, planned }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [version, ...rest] = process.argv.slice(2)
  if (!version || rest.length > 0) {
    throw new Error('usage: node scripts/release/bump-version.mjs VERSION')
  }

  // A Windows checkout can produce CRLF; the site patterns are written against LF. Normalize for
  // matching and restore per file, so a bump never rewrites line endings as a side effect.
  const files = {}
  const crlf = new Set()
  for (const path of BUMPED_FILES) {
    const raw = await readFile(join(root, path), 'utf8')
    if (raw.includes('\r\n')) crlf.add(path)
    files[path] = crlf.has(path) ? raw.replaceAll('\r\n', '\n') : raw
  }

  const { previous, planned } = planBump(version, files, process.env.RELEASE_DATE)
  const written = []
  for (const [path, content] of planned) {
    if (content === files[path]) continue
    await writeFile(join(root, path), crlf.has(path) ? content.replaceAll('\n', '\r\n') : content)
    written.push(path)
  }

  console.log(`bumped ${previous} -> ${version} across ${written.length} files`)
  for (const path of written) console.log(`  ${path}`)
  console.log('\nnext: npm run check:versions && git diff')
}
