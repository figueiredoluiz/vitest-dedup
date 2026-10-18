// Resolves the native binary for the running host and hands it the argv it was given.
//
// The launcher is the only part of the npm distribution a user runs directly, so its two jobs are
// both about not lying:
//
//   1. **Exit codes are a documented contract** (0 clean, 1 quality gate failed, 2 operational
//      error). The child's status is forwarded verbatim; every failure *this file* is responsible
//      for -- an unsupported platform, a missing optional package, an unexecutable file, a spawn
//      error -- reports 2, because none of them is a statement about duplicate tests. A launcher
//      that let a resolution failure surface as an uncaught exception would exit 1 and be
//      indistinguishable from "your tests are duplicated".
//
//   2. **A failure names what failed.** A stack trace through `createRequire` tells a user nothing
//      actionable. Every path out of here writes one `vitest-dedup: ...` line naming the platform,
//      the package, or the file involved.
//
// Platform resolution is delegated to `targetFor()` in `./targets.mjs`, which reads
// `../prebuilt-targets.json` -- the same manifest the release matrix, the platform packages, and
// the GitHub Action are built from. A second copy of the platform list here is exactly the drift
// that manifest exists to prevent.
import { accessSync, constants, existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { spawnSync } from 'node:child_process'
import { constants as osConstants } from 'node:os'
import { dirname, resolve } from 'node:path'

import { TARGETS, targetFor as resolveTarget } from './targets.mjs'

const require = createRequire(import.meta.url)

export { TARGETS }

/**
 * Distinguishes a glibc host from a musl one.
 *
 * `process.report` exposes the runtime's glibc version when it has one; musl builds omit the
 * field. Anything that throws (a hardened runtime denying the report, a future Node removing it)
 * falls back to musl, which is the statically linked build and therefore the one that runs
 * correctly in both places.
 */
export function detectLibc(report = process.report) {
  try {
    return report?.getReport()?.header?.glibcVersionRuntime ? 'gnu' : 'musl'
  } catch {
    return 'musl'
  }
}

/**
 * Resolves the running host to a release target, or throws naming the platform.
 *
 * `targetFor()` in targets.mjs returns `undefined` rather than guessing. That is the right shape
 * for a pure lookup and the wrong shape for a user-facing failure, so the message is built here.
 */
export function targetFor(
  platform = process.platform,
  arch = process.arch,
  libc = platform === 'linux' ? detectLibc() : undefined,
) {
  const target = resolveTarget(platform, arch, libc)
  if (!target) {
    const host = platform === 'linux' ? `${platform}-${arch}-${libc}` : `${platform}-${arch}`
    throw new Error(
      `unsupported platform ${host}; vitest-dedup publishes prebuilt binaries for ` +
        `${Object.keys(TARGETS).join(', ')}. Build from source with ` +
        '`cargo install vitest-dedup` instead.',
    )
  }
  return target
}

/**
 * Locates the executable this host should run.
 *
 * `VITEST_DEDUP_BINARY` short-circuits everything: it is how a developer runs a
 * `cargo build`-produced binary through the npm entry point, and how the local install simulation
 * in `npm/test/` exercises the real forwarding path without publishing anything.
 */
export function resolveBinary({
  platform = process.platform,
  arch = process.arch,
  libc = platform === 'linux' ? detectLibc() : undefined,
  override = process.env.VITEST_DEDUP_BINARY,
  resolvePackage = require.resolve,
} = {}) {
  if (override) {
    const binary = resolve(override)
    assertUsableBinary(binary, platform)
    return binary
  }

  const target = targetFor(platform, arch, libc)
  let manifest
  try {
    manifest = resolvePackage(`${target.packageName}/package.json`)
  } catch (error) {
    throw new Error(
      `the prebuilt binary package ${target.packageName} is not installed. Reinstall with ` +
        'optional dependencies enabled (`npm install vitest-dedup`, without --no-optional / ' +
        '--omit=optional), or point VITEST_DEDUP_BINARY at a locally built binary.',
      { cause: error },
    )
  }
  const binary = resolve(dirname(manifest), 'bin', target.binaryName)
  assertUsableBinary(binary, platform)
  return binary
}

// A package that resolved but has no runnable file in it is a broken install, not an unsupported
// platform, and saying so is the difference between "reinstall" and "file a bug".
function assertUsableBinary(binary, platform) {
  if (!existsSync(binary)) {
    throw new Error(`no vitest-dedup binary at ${binary}`)
  }
  if (platform !== 'win32') {
    try {
      accessSync(binary, constants.X_OK)
    } catch (error) {
      throw new Error(`the vitest-dedup binary at ${binary} is not executable`, { cause: error })
    }
  }
}

/**
 * Runs the native binary and returns the exit code the launcher process should adopt.
 *
 * `stdio: 'inherit'` keeps the child attached to the real streams, which matters more than it
 * looks: the `terminal` and `jsonl` reporters both write to stdout and downstream consumers pipe
 * them. Buffering and re-emitting would break `--reporters jsonl | jq` and destroy TTY detection.
 */
export function run(args, { binary = undefined, spawn = spawnSync, stderr = process.stderr } = {}) {
  let resolved
  try {
    resolved = binary ?? resolveBinary()
  } catch (error) {
    stderr.write(`vitest-dedup: ${error.message}\n`)
    return 2
  }

  const result = spawn(resolved, args, { stdio: 'inherit' })
  if (result.error) {
    stderr.write(`vitest-dedup: failed to start ${resolved}: ${result.error.message}\n`)
    return 2
  }
  // A signalled child has no exit status. 128+signal is the shell convention, so Ctrl-C through
  // the launcher reports 130 exactly as it would running the binary directly.
  if (result.signal) {
    stderr.write(`vitest-dedup: the analyzer was terminated by ${result.signal}\n`)
    return 128 + (osConstants.signals[result.signal] ?? 0)
  }
  // `status` is null only in states already handled above; treating a null as 2 rather than 0
  // keeps an unknown outcome from being reported as a clean run.
  return result.status ?? 2
}
