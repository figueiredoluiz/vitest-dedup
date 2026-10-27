#!/usr/bin/env node
// Recover npm's native inputs from the authenticated release, without rebuilding executables.
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { TARGETS } from '../../npm/lib/targets.mjs'
import { validateRecoveryRelease } from './check-recovery-release.mjs'
import { parseChecksumFile } from './verify-npm-binaries.mjs'

const MAX_BYTES = 128 * 1024 * 1024

/**
 * Download and authenticate every target for the exact release identity, then expose its binaries
 * in the artifact layout prepare-npm-packages consumes. Refuse existing output and remove partial
 * staging on failure. Only the expected regular file is read from each archive, through stdout.
 */
export function recoverNpmBinaries({ version, repo, sha, output, run = execFileSync }) {
  if (!/^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?$/.test(version) ||
      !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo) ||
      repo.startsWith('-') || !/^[0-9a-f]{40}$/.test(sha)) {
    throw new Error('invalid recovery version, repository or source commit')
  }
  output = resolve(output)
  if (lstatSync(output, { throwIfNoEntry: false })) throw new Error('recovery output already exists')
  const options = { timeout: 60_000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] }
  const release = run('gh', ['release', 'view', `v${version}`, '--repo', repo, '--json', 'tagName,isDraft'], options)
  validateRecoveryRelease(JSON.parse(release.toString()), `v${version}`)

  mkdirSync(dirname(output), { recursive: true })
  const scratch = mkdtempSync(join(dirname(output), '.npm-recovery-'))
  const native = join(scratch, 'native')
  const checked = []
  try {
    mkdirSync(native)
    for (const [key, target] of Object.entries(TARGETS)) {
      const stem = `vitest-dedup-${version}-${key}`
      const name = `${stem}.tar.gz`
      const archive = join(scratch, name)
      run('gh', ['release', 'download', `v${version}`, '--repo', repo,
        '--pattern', name, '--pattern', `${name}.sha256`, '--dir', scratch], options)
      if (statSync(archive).size > MAX_BYTES || statSync(`${archive}.sha256`).size > 4096) {
        throw new Error(`release input too large for ${key}`)
      }
      const { digest, name: checksumName } = parseChecksumFile(readFileSync(`${archive}.sha256`, 'utf8'))
      if (checksumName !== name || createHash('sha256').update(readFileSync(archive)).digest('hex') !== digest) {
        throw new Error(`checksum mismatch for ${key}`)
      }
      run('gh', ['attestation', 'verify', archive,
        '--repo', repo, '--signer-workflow', `${repo}/.github/workflows/release.yml`,
        '--source-ref', `refs/tags/v${version}`, '--source-digest', sha], options)

      const member = `${stem}/${target.binaryName}`
      const listing = run('tar', ['-tzf', archive, '--', member], options).toString().trim().split(/\r?\n/)
      const types = run('tar', ['-tvzf', archive, '--', member], options).toString().trim().split(/\r?\n/)
      if (listing.length !== 1 || listing[0] !== member || types.length !== 1 || !types[0].startsWith('-')) {
        throw new Error(`expected one regular binary member for ${key}`)
      }
      const bytes = run('tar', ['-xOzf', archive, '--', member], { ...options, maxBuffer: MAX_BYTES })
      if (bytes.length === 0) throw new Error(`empty release binary for ${key}`)
      const directory = join(native, `binary-${target.rustTarget}`)
      mkdirSync(directory)
      writeFileSync(join(directory, target.binaryName), bytes, { mode: 0o755 })
      checked.push(key)
    }
    if (checked.length === 0) throw new Error('no recovery targets')
    renameSync(native, output)
    return checked
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 3) throw new Error('usage: recover-npm-binaries.mjs OUTPUT_DIR')
    const checked = recoverNpmBinaries({
      version: process.env.VERSION, repo: process.env.GH_REPO,
      sha: process.env.RELEASE_SHA, output: process.argv[2],
    })
    console.log(`recovered ${checked.length} authenticated release binaries`)
  } catch (error) {
    console.error(`npm recovery failed: ${error.message}`)
    process.exitCode = 1
  }
}
