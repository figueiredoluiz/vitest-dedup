#!/usr/bin/env node
// Single entry point for a version bump: `npm run bump:version -- 0.2.0`.
//
// It rewrites exactly the set of sites `version-sites.mjs` declares, which is exactly the set
// `check-release-version.mjs` verifies -- so a bump performed here always satisfies the release
// gate, and a site one of them learns about is a site both learn about.
//
// Every replacement is planned and validated before any write. If a write fails, restore the
// original bytes of every file touched so the tree cannot remain half-bumped.
//
// Cargo.lock is rewritten directly rather than by invoking `cargo update`. This crate is the only
// workspace member, so its lock entry is a single well-known line; rewriting it keeps the bump
// instant, offline, and free of the serialized-cargo constraint this repository works under.
// cuke-dedup shells out to cargo because it has a second (fuzz) lockfile whose dependency graph
// really does need resolving.
import { readFile, writeFile } from 'node:fs/promises'
import assert from 'node:assert/strict'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  ACTION_VERSION_TAG,
  CARGO_LOCK_VERSION,
  CARGO_PACKAGE_VERSION,
  README_ACTION_VERSION,
  RELEASE_WORKFLOW_DEFAULT,
  SEMVER,
  cargoVersion,
  releaseWorkflowVersion,
} from './version-sites.mjs'
import { TARGETS } from '../../npm/lib/targets.mjs'
import { platformManifest } from './platform-package.mjs'

const targets = Object.values(TARGETS)
const platformFiles = targets.map((target) => `npm/platforms/${target.packageDirectory}/package.json`)
export const BUMPED_FILES = ['Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json',
  'README.md', '.github/workflows/release.yml', ...platformFiles]

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

  // npm manifests and its lockfile use two-space JSON; a round-trip preserves their formatting.
  const manifest = JSON.parse(files['package.json'])
  assert.equal(manifest.version, previous, 'package.json differs from Cargo.toml before the bump')
  assert.deepEqual(manifest.workspaces, ['npm/platforms/*'])
  assert.deepEqual(Object.keys(manifest.optionalDependencies).sort(), targets.map((target) => target.packageName).sort())
  manifest.version = version
  for (const name of Object.keys(manifest.optionalDependencies ?? {})) {
    manifest.optionalDependencies[name] = version
  }
  planned.set('package.json', `${JSON.stringify(manifest, null, 2)}\n`)

  const lock = JSON.parse(files['package-lock.json'])
  assert.equal(lock.version, previous, 'package-lock.json differs from Cargo.toml before the bump')
  assert.equal(lock.packages?.['']?.version, previous)
  assert.deepEqual(lock.packages?.['']?.workspaces, manifest.workspaces)
  assert.deepEqual(lock.packages?.['']?.optionalDependencies,
    Object.fromEntries(targets.map((target) => [target.packageName, previous])))
  lock.version = version
  lock.packages[''].version = version
  for (const target of targets) {
    const path = `npm/platforms/${target.packageDirectory}/package.json`
    const workspace = JSON.parse(files[path])
    assert.deepEqual(workspace, platformManifest(target, previous), `${path} differs from the target definition`)
    workspace.version = version
    planned.set(path, `${JSON.stringify(workspace, null, 2)}\n`)
    const lockPath = `npm/platforms/${target.packageDirectory}`
    assert.equal(lock.packages?.[lockPath]?.version, previous, `${lockPath} is stale`)
    assert.deepEqual(lock.packages?.[`node_modules/${target.packageName}`],
      { resolved: lockPath, link: true }, `${lockPath} is not a local link`)
    lock.packages[lockPath].version = version
    lock.packages[''].optionalDependencies[target.packageName] = version
  }
  planned.set('package-lock.json', `${JSON.stringify(lock, null, 2)}\n`)

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
  const workflow = files['.github/workflows/release.yml']
  assert.equal(releaseWorkflowVersion(workflow), previous,
    'release workflow default differs from Cargo.toml before the bump')
  planned.set('.github/workflows/release.yml',
    replaceOnce(workflow, RELEASE_WORKFLOW_DEFAULT, '.github/workflows/release.yml', `$1${version}`))

  return { previous, planned }
}

export async function applyBump(version, { rootDir = root, read = readFile, write = writeFile } = {}) {
  // A Windows checkout can produce CRLF; the site patterns are written against LF. Normalize for
  // matching and restore per file, so a bump never rewrites line endings as a side effect.
  const files = {}
  const rawFiles = {}
  const crlf = new Set()
  for (const path of BUMPED_FILES) {
    const raw = await read(join(rootDir, path), 'utf8')
    rawFiles[path] = raw
    if (raw.includes('\r\n')) crlf.add(path)
    files[path] = crlf.has(path) ? raw.replaceAll('\r\n', '\n') : raw
  }

  const { previous, planned } = planBump(version, files)
  const written = []
  try {
    for (const [path, content] of planned) {
      const next = crlf.has(path) ? content.replaceAll('\n', '\r\n') : content
      if (next === rawFiles[path]) continue
      written.push(path)
      await write(join(rootDir, path), next)
    }
  } catch (error) {
    const rollbackErrors = []
    for (const path of [...written].reverse()) {
      try {
        await write(join(rootDir, path), rawFiles[path])
      } catch (rollbackError) {
        rollbackErrors.push(`${path}: ${rollbackError}`)
      }
    }
    throw new Error(`version bump failed: ${error}; rollback errors: ${rollbackErrors.join('; ') || 'none'}`)
  }

  return { previous, written }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [version, ...rest] = process.argv.slice(2)
  if (!version || rest.length > 0) {
    throw new Error('usage: node scripts/release/bump-version.mjs VERSION')
  }
  const { previous, written } = await applyBump(version)
  console.log(`bumped ${previous} -> ${version} across ${written.length} files`)
  for (const path of written) console.log(`  ${path}`)
  console.log('\nnext: npm run check:versions && git diff')
}
