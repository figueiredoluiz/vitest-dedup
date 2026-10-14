// Tracked workspace manifests must agree with the target list, npm's metadata vocabulary, and
// the launcher's optional dependencies.
import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtemp, readFile, writeFile, chmod, mkdir, rm } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { TARGETS } from '../lib/targets.mjs'
import { platformManifest, renderPlatformReadme } from '../../scripts/release/platform-package.mjs'
import { prepareAll, writePlatformPackage } from '../../scripts/release/prepare-npm-packages.mjs'

const rootManifest = JSON.parse(await readFile(new URL('../../package.json', import.meta.url), 'utf8'))

/** Write one target's workspace manifest beneath root and return its package directory. */
async function writeWorkspaceFixture(root, target, version) {
  const directory = join(root, 'npm', 'platforms', target.packageDirectory)
  await mkdir(directory, { recursive: true })
  await writeFile(join(directory, 'package.json'),
    `${JSON.stringify(platformManifest(target, version), null, 2)}\n`)
  return directory
}

/** Create manifests and mock binary artifacts for every target using its requested fixture version. */
async function writeCompleteArtifactSet(root, artifacts, versionForTarget) {
  for (const target of Object.values(TARGETS)) {
    await writeWorkspaceFixture(root, target, versionForTarget(target))
    const directory = join(artifacts, `binary-${target.rustTarget}`)
    await mkdir(directory, { recursive: true })
    await writeFile(join(directory, target.binaryName), 'native binary\n')
  }
}

/** Return temporary workspace/artifact roots with a LICENSE and complete target set; register both for cleanup. */
async function completeSetFixture(t, versionForTarget) {
  const root = await mkdtemp(join(tmpdir(), 'vitest-dedup-complete-set-'))
  const artifacts = await mkdtemp(join(tmpdir(), 'vitest-dedup-complete-artifacts-'))
  t.after(() => Promise.all([root, artifacts].map((path) => rm(path, { recursive: true, force: true }))))
  await writeFile(join(root, 'LICENSE'), 'MIT\n')
  await writeCompleteArtifactSet(root, artifacts, versionForTarget)
  return { root, artifacts }
}

test('a tracked workspace manifest exists for every target and nothing else', async () => {
  const names = Object.values(TARGETS).map((target) => target.packageName)
  assert.equal(names.length, 8)
  assert.deepEqual(rootManifest.workspaces, ['npm/platforms/*'])
  assert.deepEqual(Object.keys(rootManifest.optionalDependencies).sort(), [...names].sort())
  for (const target of Object.values(TARGETS)) {
    assert.equal(rootManifest.optionalDependencies[target.packageName], rootManifest.version)
    const manifest = JSON.parse(await readFile(
      new URL(`../../npm/platforms/${target.packageDirectory}/package.json`, import.meta.url), 'utf8'))
    assert.deepEqual(manifest, platformManifest(target, rootManifest.version))
  }
})

test('generated manifests use npm metadata, not Rust target vocabulary', () => {
  for (const target of Object.values(TARGETS)) {
    const manifest = platformManifest(target, '1.2.3')
    assert.equal(manifest.name, target.packageName)
    assert.equal(manifest.version, '1.2.3')
    assert.deepEqual(manifest.os, [target.platform])
    assert.deepEqual(manifest.cpu, [target.arch])
    // npm spells these `glibc`/`musl`; the manifest spells them `gnu`/`musl` because that is the
    // Rust triple. Publishing `gnu` here makes npm match no host at all.
    if (target.libc === 'gnu') assert.deepEqual(manifest.libc, ['glibc'])
    else if (target.libc === 'musl') assert.deepEqual(manifest.libc, ['musl'])
    else assert.equal(manifest.libc, undefined)
    assert.deepEqual(manifest.files, [`bin/${target.binaryName}`, 'README.md', 'LICENSE'])
    assert.equal(manifest.license, 'MIT')
    // A platform package that pulled anything at install time would defeat the whole design.
    assert.equal(manifest.dependencies, undefined)
    assert.equal(manifest.scripts, undefined)
  }
})

test('an unknown libc is refused rather than guessed', () => {
  assert.throws(
    () => platformManifest({ ...TARGETS['linux-x64-gnu'], libc: 'uclibc' }, '1.0.0'),
    /no npm libc value/,
  )
})

test('the platform README steers users to the launcher package', () => {
  const manifest = platformManifest(TARGETS['linux-x64-musl'], '1.2.3')
  const readme = renderPlatformReadme(manifest)
  assert.match(readme, /^# vitest-dedup-linux-x64-musl$/m)
  assert.match(readme, /Do not install this package directly/)
  assert.match(readme, /npm install --save-dev vitest-dedup/)
  assert.match(readme, /npm audit signatures/)
  assert.match(readme, /no install scripts/)
})

test('preparing a package writes exactly the allowlisted files, binary executable', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'vitest-dedup-prepare-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'LICENSE'), 'MIT\n')
  const source = join(root, 'built-binary')
  await writeFile(source, '#!/bin/sh\nexit 0\n')

  const target = TARGETS['linux-x64-gnu']
  await writeWorkspaceFixture(root, target, '9.9.9')
  const packageRoot = await writePlatformPackage({ root, target, version: '9.9.9', binary: source })

  const written = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))
  assert.deepEqual(written, platformManifest(target, '9.9.9'))
  for (const entry of written.files) {
    assert.ok(existsSync(join(packageRoot, entry)), `${entry} must exist`)
  }
  if (process.platform !== 'win32') {
    const { mode } = await import('node:fs/promises').then((fs) =>
      fs.stat(join(packageRoot, 'bin', target.binaryName)))
    assert.equal(mode & 0o111, 0o111, 'the published binary must be executable')
  }
})

test('an incomplete build refuses to produce a publishable set', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'vitest-dedup-partial-'))
  const artifacts = await mkdtemp(join(tmpdir(), 'vitest-dedup-artifacts-'))
  t.after(() => Promise.all([root, artifacts].map((p) => rm(p, { recursive: true, force: true }))))
  await writeFile(join(root, 'LICENSE'), 'MIT\n')
  for (const target of Object.values(TARGETS)) {
    await writeWorkspaceFixture(root, target, '9.9.9')
  }

  // One leg of the matrix succeeded; the other seven did not.
  const present = TARGETS['darwin-arm64']
  await mkdir(join(artifacts, `binary-${present.rustTarget}`), { recursive: true })
  const binary = join(artifacts, `binary-${present.rustTarget}`, present.binaryName)
  await writeFile(binary, '#!/bin/sh\nexit 0\n')
  await chmod(binary, 0o755)

  await assert.rejects(
    prepareAll({ root, artifacts, version: '9.9.9' }),
    /missing native binaries for 7 of 8 targets/,
  )
  assert.equal(existsSync(join(root, 'npm', 'platforms', present.packageDirectory, 'bin')), false,
    'a partial build must not assemble even the available target')
})

test('a complete build assembles all eight validated workspaces', async (t) => {
  const { root, artifacts } = await completeSetFixture(t, () => '9.9.9')
  const written = await prepareAll({ root, artifacts, version: '9.9.9' })
  assert.equal(written.length, 8)
  for (const target of Object.values(TARGETS)) {
    const directory = join(root, 'npm', 'platforms', target.packageDirectory)
    assert.ok(existsSync(join(directory, 'bin', target.binaryName)))
    assert.ok(existsSync(join(directory, 'README.md')))
    assert.deepEqual(JSON.parse(await readFile(join(directory, 'package.json'), 'utf8')),
      platformManifest(target, '9.9.9'))
  }
})

test('a stale later manifest refuses the whole set before any package is assembled', async (t) => {
  const { root, artifacts } = await completeSetFixture(t,
    (target) => target.packageDirectory === 'win32-x64' ? '0.0.0' : '9.9.9')
  await assert.rejects(prepareAll({ root, artifacts, version: '9.9.9' }),
    /workspace manifest differs/)
  assert.equal(existsSync(join(root, 'npm', 'platforms', 'darwin-arm64', 'bin')), false,
    'an earlier platform must remain untouched when a later manifest is stale')
})

test('a stale workspace manifest is refused before package assembly', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'vitest-dedup-stale-workspace-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'LICENSE'), 'MIT\n')
  const target = TARGETS['linux-x64-gnu']
  const directory = await writeWorkspaceFixture(root, target, '0.0.0')
  const binary = join(root, 'built-binary')
  await writeFile(binary, '#!/bin/sh\nexit 0\n')
  await assert.rejects(writePlatformPackage({ root, target, version: '9.9.9', binary }),
    /workspace manifest differs/)
  assert.equal(existsSync(join(directory, 'bin')), false)
})
