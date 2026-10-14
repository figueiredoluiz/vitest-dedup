// The npm binaries and the GitHub release archives are separate artifacts of the same build.
// Only this check ties them together, so these tests build both sides for real -- a genuine
// gzipped tarball and a genuine sha256sum file -- and then break each link in turn.
import assert from 'node:assert/strict'
import test from 'node:test'
import { execFileSync, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises'
import { readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { TARGETS } from '../lib/targets.mjs'
import { parseChecksumFile, verifyNpmBinaries } from '../../scripts/release/verify-npm-binaries.mjs'

const tarAvailable = spawnSync('tar', ['--version'], { stdio: 'ignore' }).status === 0

test('checksum lines are parsed in both GNU and BSD spellings', () => {
  const digest = 'a'.repeat(64)
  assert.deepEqual(parseChecksumFile(`${digest}  vitest-dedup-0.1.0-linux-x64-gnu.tar.gz\n`), {
    digest,
    name: 'vitest-dedup-0.1.0-linux-x64-gnu.tar.gz',
  })
  assert.deepEqual(parseChecksumFile(`${digest} *archive.tar.gz`), {
    digest,
    name: 'archive.tar.gz',
  })
  assert.throws(() => parseChecksumFile('not a checksum'), /unparseable checksum file/)
})

/**
 * Build eight raw binaries and checksummed archives for version, optionally corrupting one archived binary.
 * Return the binary/archive roots and register the temporary release tree for test cleanup.
 */
async function releaseFixture(t, version, { corrupt } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'vitest-dedup-release-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const binaries = join(root, 'binaries')
  const archives = join(root, 'archives')
  const staging = join(root, 'staging')
  await mkdir(archives, { recursive: true })

  for (const [key, target] of Object.entries(TARGETS)) {
    const content = `binary for ${key}\n`
    await mkdir(join(binaries, `binary-${target.rustTarget}`), { recursive: true })
    await writeFile(join(binaries, `binary-${target.rustTarget}`, target.binaryName), content)

    const stem = `vitest-dedup-${version}-${key}`
    await mkdir(join(staging, stem), { recursive: true })
    await writeFile(
      join(staging, stem, target.binaryName),
      corrupt === key ? 'a different binary\n' : content,
    )
    execFileSync('tar', ['-czf', join(archives, `${stem}.tar.gz`), '-C', staging, stem])
    const digest = createHash('sha256')
      .update(readFileSync(join(archives, `${stem}.tar.gz`)))
      .digest('hex')
    await writeFile(join(archives, `${stem}.tar.gz.sha256`), `${digest}  ${stem}.tar.gz\n`)
  }
  return { binaries, archives }
}

test('a consistent release verifies every target', async (t) => {
  if (!tarAvailable) return t.skip('tar is unavailable')
  const { binaries, archives } = await releaseFixture(t, '0.1.0')
  assert.equal(verifyNpmBinaries({ binaries, archives, version: '0.1.0' }).length, 8)
})

test('an npm binary that differs from its archived twin is caught', async (t) => {
  if (!tarAvailable) return t.skip('tar is unavailable')
  const { binaries, archives } = await releaseFixture(t, '0.1.0', { corrupt: 'linux-x64-musl' })
  assert.throws(
    () => verifyNpmBinaries({ binaries, archives, version: '0.1.0' }),
    /the npm binary for linux-x64-musl differs from the one in the checksummed release archive/,
  )
})

test('a tampered archive is caught even though its checksum file travels with it', async (t) => {
  if (!tarAvailable) return t.skip('tar is unavailable')
  const { binaries, archives } = await releaseFixture(t, '0.1.0')
  await writeFile(join(archives, 'vitest-dedup-0.1.0-darwin-x64.tar.gz'), 'not a tarball\n')
  assert.throws(
    () => verifyNpmBinaries({ binaries, archives, version: '0.1.0' }),
    /checksum mismatch for vitest-dedup-0\.1\.0-darwin-x64\.tar\.gz/,
  )
})

test('a missing target is a failure, not a smaller run', async (t) => {
  if (!tarAvailable) return t.skip('tar is unavailable')
  const { binaries, archives } = await releaseFixture(t, '0.1.0')
  await rm(join(archives, 'vitest-dedup-0.1.0-win32-arm64.tar.gz'))
  assert.throws(
    () => verifyNpmBinaries({ binaries, archives, version: '0.1.0' }),
    /missing release input for win32-arm64/,
  )
})

test('a version mismatch between the tag and the archives is caught', async (t) => {
  if (!tarAvailable) return t.skip('tar is unavailable')
  const { binaries, archives } = await releaseFixture(t, '0.1.0')
  assert.throws(
    () => verifyNpmBinaries({ binaries, archives, version: '0.2.0' }),
    /missing release input for/,
  )
})
