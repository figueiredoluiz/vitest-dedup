// The release and CI workflows are configuration, so nothing type-checks them and nothing runs
// them until a tag is pushed. These assertions pin the properties whose violation is silent and
// expensive: publishing out of order, publishing with a long-lived token, or shipping a gate that
// no workflow invokes.
import assert from 'node:assert/strict'
import test from 'node:test'
import { readFile } from 'node:fs/promises'

import { TARGETS } from '../lib/targets.mjs'

const release = await readFile(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8')
const ci = await readFile(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8')
const rootManifest = JSON.parse(await readFile(new URL('../../package.json', import.meta.url), 'utf8'))

test('npm publishing runs only after the GitHub release exists', () => {
  const job = release.slice(release.indexOf('\n  publish-npm:'))
  assert.ok(job.length > 0, 'release.yml has no publish-npm job')
  assert.match(job, /needs: \[resolve, build, publish\]/)
  // A single script owns the platform-first ordering; the workflow must not reimplement it as a
  // sequence of steps that a later edit could reorder.
  assert.match(job, /node scripts\/release\/publish-npm-packages\.mjs/)
  assert.doesNotMatch(job, /npm publish/)
})

test('only the repository owner can publish or rerun a publication', () => {
  const publishStart = release.indexOf('\n  publish:')
  const npmStart = release.indexOf('\n  publish-npm:')
  assert.ok(publishStart > -1 && npmStart > publishStart, 'release.yml has no bounded publish jobs')

  const githubReleaseJob = release.slice(publishStart, npmStart)
  const npmReleaseJob = release.slice(npmStart)
  const ownerGate =
    /github\.actor == github\.repository_owner && github\.triggering_actor == github\.repository_owner/
  assert.match(githubReleaseJob, ownerGate)
  assert.match(npmReleaseJob, ownerGate)
  assert.match(npmReleaseJob, /vars\.PUBLISH_NPM == 'true'/)
})

test('npm publishing authenticates with OIDC and never a stored token', () => {
  const job = release.slice(release.indexOf('\n  publish-npm:'))
  assert.match(job, /permissions:\s*\n\s*contents: read\s*\n\s*id-token: write/)
  assert.match(job, /check-npm-trusted-publishing\.mjs/)
  // Matched against usage, not prose: the job's own comment explains why no token exists, and a
  // check that tripped over the explanation would have to be deleted rather than satisfied.
  assert.doesNotMatch(release, /NODE_AUTH_TOKEN:/)
  assert.doesNotMatch(release, /\$\{\{\s*secrets\./)
})

test('the npm binaries are tied to the attested release archives before publishing', () => {
  const job = release.slice(release.indexOf('\n  publish-npm:'))
  assert.match(job, /pattern: binary-\*/)
  assert.match(job, /pattern: release-\*/)
  assert.match(job, /verify-npm-binaries\.mjs/)
  // Both artifact kinds have to be produced by the build job, or the publish job downloads nothing.
  assert.match(release, /name: binary-\$\{\{ matrix\.rustTarget \}\}/)
  assert.match(release, /name: release-\$\{\{ matrix\.key \}\}/)
})

test('platform packages are generated at release time, never read from the repository', () => {
  assert.match(release, /prepare-npm-packages\.mjs native/)
  // `npm/platforms/` is build output. A committed manifest there is a second platform list.
  assert.doesNotMatch(release, /npm\/platforms\/[a-z0-9-]+\/package\.json/)
})

test('the version gate runs before anything is published', () => {
  const job = release.slice(release.indexOf('\n  publish-npm:'))
  const gate = job.indexOf('check-release-version.mjs')
  const publish = job.indexOf('publish-npm-packages.mjs')
  assert.ok(gate > -1 && publish > -1 && gate < publish, 'versions must be checked before publish')
})

test('every optional dependency corresponds to a target the release actually builds', () => {
  assert.deepEqual(
    Object.keys(rootManifest.optionalDependencies).sort(),
    Object.values(TARGETS).map((target) => target.packageName).sort(),
  )
})

test('CI runs the npm suite, the version gate, and the install simulation', () => {
  assert.match(ci, /run: npm test/)
  assert.match(ci, /run: npm run check:versions/)
  assert.match(ci, /run: npm run pack:check/)
  assert.match(ci, /install-simulation\.test\.mjs/)
  // The simulation must be unable to degrade into a skip on the job that exists to run it.
  assert.match(ci, /VITEST_DEDUP_REQUIRE_INSTALL_SIM: "1"/)
  // Windows exercises the `.exe` binary name and npm's .cmd bin shim.
  assert.match(ci, /os: \[ubuntu-22\.04, windows-latest\]/)
})

test('the JavaScript coverage job provisions gitleaks before running scanner tests', () => {
  const start = ci.indexOf('\n  quality-gates:')
  const end = ci.indexOf('\n  memory:', start)
  const job = ci.slice(start, end)
  assert.ok(start > -1 && end > start, 'ci.yml has no bounded quality-gates job')
  const install = job.indexOf('name: Install gitleaks')
  const coverage = job.indexOf('npm run check:coverage:js')
  assert.ok(install > -1, 'the coverage suite exercises the secret scanner but installs no gitleaks')
  assert.ok(install < coverage, 'gitleaks must be available before the coverage suite starts')
  const setup = job.slice(install, coverage)
  assert.match(setup, /GITLEAKS_VERSION: \d+\.\d+\.\d+/)
  assert.match(setup, /GITLEAKS_SHA256: [a-f0-9]{64}/)
  assert.match(setup, /sha256sum --check --strict/)
  assert.match(setup, /GITHUB_PATH/)
})

test('every npm script CI invokes exists in package.json', () => {
  const invoked = [...ci.matchAll(/run: npm run ([a-z:-]+)/g)].map((match) => match[1])
  assert.ok(invoked.length >= 2, `found ${invoked.length} npm script invocations in ci.yml`)
  for (const name of invoked) {
    assert.ok(rootManifest.scripts[name], `ci.yml runs "npm run ${name}", which is not defined`)
  }
})
