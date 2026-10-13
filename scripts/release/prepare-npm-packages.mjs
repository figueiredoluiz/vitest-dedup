// Adds native binaries and package documents to checked-in platform workspaces. The tracked
// manifests are validated against `npm/prebuilt-targets.json`, never overwritten.
//
// Two modes:
//
//   node scripts/release/prepare-npm-packages.mjs <artifacts-dir>
//       Release mode. Expects `<artifacts-dir>/binary-<rustTarget>/<binaryName>` -- the layout
//       `actions/download-artifact` produces from the `binary-*` artifacts the build job uploads.
//       Every target must be present; a missing one means a matrix leg failed and publishing that
//       set would leave a platform resolvable in `optionalDependencies` but absent from the
//       registry, which breaks installs on that platform rather than degrading.
//
//   node scripts/release/prepare-npm-packages.mjs --host <binary>
//       Local mode, for the install simulation in `npm/test/` and for trying the launcher against
//       a `cargo build --release` binary. Writes only the package matching the running host.
import assert from 'node:assert/strict'
import { chmod, copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { TARGETS, targetFor } from '../../npm/lib/targets.mjs'
import { detectLibc } from '../../npm/lib/launcher.mjs'
import { platformManifest, renderPlatformReadme } from './platform-package.mjs'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

async function validatedManifest({ root, target, version }) {
  const packageRoot = join(root, 'npm', 'platforms', target.packageDirectory)
  const manifest = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))
  assert.deepEqual(
    manifest,
    platformManifest(target, version),
    `${target.packageName} workspace manifest differs from the target definition or version`,
  )
  return manifest
}

/** Adds files to a package whose tracked manifest was already validated. */
async function assemblePlatformPackage({ root, target, manifest, binary }) {
  const packageRoot = join(root, 'npm', 'platforms', target.packageDirectory)
  await mkdir(join(packageRoot, 'bin'), { recursive: true })
  await writeFile(join(packageRoot, 'README.md'), renderPlatformReadme(manifest))
  await copyFile(join(root, 'LICENSE'), join(packageRoot, 'LICENSE'))

  const destination = join(packageRoot, 'bin', target.binaryName)
  await copyFile(binary, destination)
  // The mode bit does not survive every artifact round-trip, and npm preserves whatever it finds.
  // A published binary without the execute bit fails at the user's first invocation, not ours.
  if (!target.binaryName.endsWith('.exe')) {
    await chmod(destination, 0o755)
  }
  return packageRoot
}

/** Validate and assemble one platform package for local install simulation. */
export async function writePlatformPackage({ root, target, version, binary }) {
  const manifest = await validatedManifest({ root, target, version })
  return assemblePlatformPackage({ root, target, manifest, binary })
}

/** Release mode: every target, sourced from the downloaded `binary-*` artifacts. */
export async function prepareAll({ root = repoRoot, artifacts, version }) {
  const directories = (await readdir(join(root, 'npm', 'platforms'), { withFileTypes: true }))
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort()
  const expected = Object.values(TARGETS).map((target) => target.packageDirectory).sort()
  assert.deepEqual(directories, expected, 'platform workspaces differ from release targets')

  const missing = []
  for (const target of Object.values(TARGETS)) {
    const binary = join(artifacts, `binary-${target.rustTarget}`, target.binaryName)
    if (!existsSync(binary)) {
      missing.push(`${target.packageName}: expected ${binary}`)
      continue
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `missing native binaries for ${missing.length} of ${Object.keys(TARGETS).length} targets; ` +
        'publishing an incomplete set would break installs on the missing platforms:\n  ' +
        missing.join('\n  '),
    )
  }
  const validated = []
  for (const target of Object.values(TARGETS)) {
    validated.push({ target, manifest: await validatedManifest({ root, target, version }) })
  }
  const written = []
  for (const { target, manifest } of validated) {
    const binary = join(artifacts, `binary-${target.rustTarget}`, target.binaryName)
    written.push(await assemblePlatformPackage({ root, target, manifest, binary }))
  }
  return written
}

async function rootVersion(root) {
  const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
  return manifest.version
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [mode, argument] = process.argv.slice(2)
  const version = await rootVersion(repoRoot)

  if (mode === '--host') {
    if (!argument) {
      throw new Error('usage: prepare-npm-packages.mjs --host <path-to-binary>')
    }
    // The same libc detection the launcher performs at run time: on a musl host (e.g. Alpine) the
    // launcher resolves the musl package, so preparing the gnu one here would leave the install
    // simulation pointing at a package the launcher never looks for.
    const target = targetFor(
      process.platform,
      process.arch,
      process.platform === 'linux' ? detectLibc() : undefined,
    )
    if (!target) {
      throw new Error(`no release target for ${process.platform}-${process.arch}`)
    }
    const written = await writePlatformPackage({
      root: repoRoot,
      target,
      version,
      binary: resolve(argument),
    })
    console.log(`prepared ${target.packageName} ${version} in ${written}`)
  } else if (mode && !mode.startsWith('--')) {
    const written = await prepareAll({ artifacts: resolve(mode), version })
    console.log(`prepared ${written.length} platform packages at ${version}`)
    for (const path of written) console.log(`  ${path}`)
  } else {
    throw new Error(
      'usage: prepare-npm-packages.mjs <artifacts-dir> | --host <path-to-binary>',
    )
  }
}
