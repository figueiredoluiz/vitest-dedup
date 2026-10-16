// Publishes the npm distribution: the eight platform packages first, the launcher package last.
//
// The order is not a preference. `vitest-dedup` lists every platform package in
// `optionalDependencies`; npm resolves those at install time, so a launcher published before its
// binaries exist installs with no binary at all for the window in between. Publishing is also not
// reversible -- npm unpublish is restricted after 72 hours -- so the ordering is enforced here
// rather than trusted to step order in a YAML file, and the launcher is refused outright if any
// platform package is not confirmed present in the registry first.
//
// Every publish is idempotent. A rerun after a partial failure (one leg 403s, the network drops
// between package five and six) skips what is already at the target version and continues, instead
// of failing on `EPUBLISHCONFLICT` and leaving the set half-published with no way forward.
import { spawnSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { TARGETS } from '../../npm/lib/targets.mjs'

const defaultRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

/**
 * Classifies an `npm view <pkg>@<version> version --json` result.
 *
 * Fails closed. Only a clean 404 counts as "not published yet"; a 401, a rate limit, a proxy error
 * or anything else throws, because "the registry did not answer" must never be read as "this
 * version is missing" -- that is how a republish attempt or an unnoticed auth failure turns into a
 * confusing hard error halfway through the set.
 */
export function registryVersionState(result, expectedVersion) {
  if (result.error) {
    throw result.error
  }

  if (result.status === 0) {
    let published
    try {
      published = JSON.parse(result.stdout)
    } catch (error) {
      throw new Error('npm returned invalid JSON while checking a published version', {
        cause: error,
      })
    }
    if (published !== expectedVersion) {
      throw new Error(
        `npm reported version ${JSON.stringify(published)} while checking ${expectedVersion}`,
      )
    }
    return 'published'
  }

  const output = `${result.stdout ?? ''}\n${result.stderr ?? ''}`
  if (output.includes('E404')) {
    return 'missing'
  }

  throw new Error(
    `npm view failed with exit code ${result.status}; refusing to publish without a reliable ` +
      'registry check',
  )
}

/**
 * `--access public` because a scoped-looking name defaults to restricted; `--provenance` because
 * the whole point of publishing from the tagged workflow run is that a consumer can verify where
 * the tarball came from with `npm audit signatures`. Stable versions explicitly select latest;
 * prereleases select next, leaving the stable channel alone.
 */
export function npmPublishArguments(packageDirectory, version) {
  // An explicit tag is required by npm for prereleases and versions below the registry highest.
  // Stable releases select latest; prereleases share next without replacing the stable channel.
  const tag = version.includes('-') ? 'next' : 'latest'
  return ['publish', packageDirectory, '--access', 'public', '--provenance', '--tag', tag]
}

/** Directories to publish, platform packages first and the launcher last. */
export function publishOrder(targets = TARGETS) {
  return [
    ...Object.values(targets).map(({ packageDirectory }) => `npm/platforms/${packageDirectory}`),
    '.',
  ]
}

/**
 * Publish or confirm native packages before the launcher, refusing missing optional
 * dependencies and registry/publish failures.
 */
export async function publishNpmPackages({ root = defaultRoot, runNpm = runNpmCommand } = {}) {
  const confirmed = new Set()

  for (const packageDirectory of publishOrder()) {
    const manifest = JSON.parse(
      await readFile(resolve(root, packageDirectory, 'package.json'), 'utf8'),
    )
    const spec = `${manifest.name}@${manifest.version}`

    // The launcher goes last, and only once every name it depends on is confirmed live. Without
    // this, a platform package silently skipped upstream (a matrix leg that never produced a
    // binary) would still yield a launcher that resolves to nothing on that platform.
    if (packageDirectory === '.') {
      const absent = Object.keys(manifest.optionalDependencies ?? {}).filter(
        (name) => !confirmed.has(name),
      )
      if (absent.length > 0) {
        throw new Error(
          `refusing to publish ${spec}: its optional dependencies are not in the registry yet: ` +
            `${absent.join(', ')}. Publishing the launcher first would break installs on those ` +
            'platforms.',
        )
      }
    }

    const view = runNpm(['view', spec, 'version', '--json'], { capture: true, root })
    if (registryVersionState(view, manifest.version) === 'published') {
      console.log(`skipping ${spec}: already published`)
      confirmed.add(manifest.name)
      continue
    }

    console.log(`publishing ${spec}`)
    const published = runNpm(npmPublishArguments(packageDirectory, manifest.version), {
      capture: false, root,
    })
    if (published.error) {
      throw published.error
    }
    if (published.status !== 0) {
      throw new Error(`npm publish failed for ${spec} with exit code ${published.status}`)
    }
    confirmed.add(manifest.name)
  }

  console.log(`published or confirmed ${confirmed.size} packages`)
}

/**
 * Run npm in the requested root and return its process result, capturing output only when
 * requested.
 */
export function runNpmCommand(arguments_, { capture, root }) {
  const { executable, prefixArguments } = npmInvocation()
  return spawnSync(executable, [...prefixArguments, ...arguments_], {
    cwd: root,
    encoding: 'utf8',
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit',
  })
}

/**
 * On Windows `npm` is a `.cmd` shim, and `spawnSync` without a shell cannot execute it. Invoking
 * `npm-cli.js` through the running Node binary avoids that without turning on `shell: true`, which
 * would reintroduce shell quoting for arguments that come from package metadata.
 */
export function npmInvocation({
  platform = process.platform,
  nodeExecutable = process.execPath,
  npmExecutablePath = process.env.npm_execpath,
} = {}) {
  if (platform !== 'win32') {
    return { executable: 'npm', prefixArguments: [] }
  }
  const cli = npmExecutablePath ?? resolve(dirname(nodeExecutable), 'node_modules/npm/bin/npm-cli.js')
  return { executable: nodeExecutable, prefixArguments: [cli] }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await publishNpmPackages()
}
