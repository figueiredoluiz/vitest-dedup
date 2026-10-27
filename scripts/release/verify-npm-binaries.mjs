// Proves the executables about to be published on npm are byte-for-byte the ones in the
// checksummed, provenance-attested GitHub release archives.
//
// The npm job downloads the raw binary (what goes into the platform package) and the
// matching build's archives for a normal release or rehearsal. npm-only recovery instead derives
// its binaries directly from authenticated archives in recover-npm-binaries.mjs.
// Each archive has a `.sha256` sidecar and is what the Action verifies before
// executing. Nothing otherwise connects the downloaded binary to the release. Without this
// check, "the release archive is attested" says nothing about the npm tarball -- a corrupted
// upload, a mixed-up matrix leg, or a tampered artifact between jobs would ship a different binary
// to npm users than to Action users, and both would look fine.
//
// The checksum is re-verified here too rather than trusted: an archive and a matching digest that
// travel together can only prove they were not corrupted in transit, so this recomputes both
// sides.
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

import { TARGETS } from '../../npm/lib/targets.mjs'

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex')

/** Parses a `sha256sum`/`shasum -a 256` line: `<64 hex>  <name>`. */
export function parseChecksumFile(text) {
  const match = /^([0-9a-f]{64})\s+\*?(\S.*)$/m.exec(text.trim())
  if (!match) {
    throw new Error(`unparseable checksum file contents: ${JSON.stringify(text.slice(0, 120))}`)
  }
  return { digest: match[1], name: match[2].trim() }
}

/**
 * Check every target's raw npm binary against its checksummed release archive using the supplied extractor.
 * Return all verified target keys; throw on missing inputs, checksum/name/layout mismatches,
 * extraction errors, differing binary bytes or an incomplete target set.
 * Remove each temporary extraction directory even when verification fails.
 */
export function verifyNpmBinaries({ binaries, archives, version, extract = extractWithTar }) {
  const checked = []
  for (const [key, target] of Object.entries(TARGETS)) {
    const binary = join(binaries, `binary-${target.rustTarget}`, target.binaryName)
    const stem = `vitest-dedup-${version}-${key}`
    const archive = join(archives, `${stem}.tar.gz`)
    const checksum = `${archive}.sha256`

    for (const path of [binary, archive, checksum]) {
      if (!existsSync(path)) {
        throw new Error(`missing release input for ${key}: ${path}`)
      }
    }

    const { digest, name } = parseChecksumFile(readFileSync(checksum, 'utf8'))
    if (basename(name) !== basename(archive)) {
      throw new Error(`${checksum} names ${name}, not ${basename(archive)}`)
    }
    const archiveDigest = sha256(archive)
    if (archiveDigest !== digest) {
      throw new Error(`checksum mismatch for ${basename(archive)}: ${archiveDigest} != ${digest}`)
    }

    const scratch = mkdtempSync(join(tmpdir(), 'vitest-dedup-verify-'))
    try {
      extract(archive, scratch)
      const extracted = join(scratch, stem, target.binaryName)
      if (!existsSync(extracted)) {
        throw new Error(`${basename(archive)} does not contain ${stem}/${target.binaryName}`)
      }
      if (sha256(extracted) !== sha256(binary)) {
        throw new Error(
          `the npm binary for ${key} differs from the one in the checksummed release archive`,
        )
      }
    } finally {
      rmSync(scratch, { recursive: true, force: true })
    }
    checked.push(key)
  }

  // A run that verified nothing must not report success: an empty or partial target list would
  // otherwise pass this gate while leaving every published binary unchecked.
  const expected = Object.keys(TARGETS).length
  if (checked.length !== expected) {
    throw new Error(`verified ${checked.length} of ${expected} targets`)
  }
  return checked
}

/** Extract a gzip tar archive into the given directory; propagate tar failures with captured stderr. */
function extractWithTar(archive, directory) {
  execFileSync('tar', ['-xzf', archive, '-C', directory], { stdio: ['ignore', 'ignore', 'pipe'] })
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = new Map()
  const argv = process.argv.slice(2)
  for (let index = 0; index < argv.length; index += 2) {
    args.set(argv[index].replace(/^--/, ''), argv[index + 1])
  }
  for (const required of ['binaries', 'archives', 'version']) {
    if (!args.get(required)) {
      throw new Error(
        'usage: verify-npm-binaries.mjs --binaries DIR --archives DIR --version X.Y.Z',
      )
    }
  }
  const checked = verifyNpmBinaries({
    binaries: resolve(args.get('binaries')),
    archives: resolve(args.get('archives')),
    version: args.get('version'),
  })
  console.log(`verified ${checked.length} npm binaries against their release archives`)
}
