// Fails the release before any publish attempt when the runner's npm is too old for trusted
// publishing.
//
// Trusted publishing exchanges the workflow's OIDC identity for a short-lived registry credential,
// which is why `.github/workflows/release.yml` carries no NPM_TOKEN at all. An npm below the
// minimum does not fall back to something weaker -- it simply has no credential and fails with an
// authentication error partway through the set, after some packages are already live. Checking the
// version first turns that into a clean refusal.
import { spawnSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

export const MINIMUM_TRUSTED_PUBLISHING_NPM = '11.5.1'

/**
 * Compare a trimmed npm version's major/minor/patch components with the trusted-publishing minimum.
 * Ignore prerelease/build suffixes, including prereleases of the minimum; reject unparseable versions.
 */
export function supportsTrustedPublishing(version) {
  const parsed = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(String(version).trim())
  if (!parsed) {
    return false
  }
  const actual = parsed.slice(1).map(Number)
  const minimum = MINIMUM_TRUSTED_PUBLISHING_NPM.split('.').map(Number)
  for (let index = 0; index < minimum.length; index += 1) {
    if (actual[index] !== minimum[index]) {
      return actual[index] > minimum[index]
    }
  }
  return true
}

/** Read the installed npm version through the supplied runner; throw on command failure or an unsupported version. */
export function installedNpmVersion(run = spawnSync) {
  const result = run('npm', ['--version'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  if (result.error) {
    throw result.error
  }
  if (result.status !== 0) {
    throw new Error(`npm --version failed with exit code ${result.status}: ${result.stderr.trim()}`)
  }
  const version = result.stdout.trim()
  if (!supportsTrustedPublishing(version)) {
    throw new Error(
      `npm ${version || '(unknown)'} does not support trusted publishing; ` +
        `release requires npm >= ${MINIMUM_TRUSTED_PUBLISHING_NPM}`,
    )
  }
  return version
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  console.log(`npm ${installedNpmVersion()} supports trusted publishing`)
}
