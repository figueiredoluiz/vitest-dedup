// The release and CI workflows are configuration, so nothing type-checks them and nothing runs
// them until a dispatch. These assertions pin the properties whose violation is silent and
// expensive: publishing out of order, publishing with a long-lived token, or shipping a gate that
// no workflow invokes.
import assert from 'node:assert/strict'
import test from 'node:test'
import { readFile } from 'node:fs/promises'

import { TARGETS } from '../lib/targets.mjs'

const release = await readFile(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8')
const ci = await readFile(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8')
const rootManifest = JSON.parse(await readFile(new URL('../../package.json', import.meta.url), 'utf8'))

test('release publication is manual and all jobs check out the event commit', () => {
  const trigger = release.slice(release.indexOf('\non:'), release.indexOf('\npermissions:'))
  assert.match(trigger, /workflow_dispatch:/)
  assert.doesNotMatch(trigger, /\n  push:/)
  assert.equal((release.match(/- uses: actions\/checkout@[a-f0-9]{40}/g) ?? []).length, 6)
  assert.equal((release.match(/ref: \$\{\{ github\.sha \}\}/g) ?? []).length, 6)
  assert.equal((release.match(/persist-credentials: false/g) ?? []).length, 6)
  const resolve = release.slice(release.indexOf('\n  resolve:'), release.indexOf('\n  build:'))
  assert.match(resolve, /fetch-depth: 0/)
  assert.match(resolve, /node scripts\/release\/validate-release-context\.mjs/)
  assert.ok(resolve.indexOf('validate-release-context.mjs --remote') > -1 &&
    resolve.indexOf('validate-release-context.mjs --remote') < resolve.indexOf('check-recovery-release.mjs'),
    'the live tag and main ancestry must be checked before npm-only recovery')
  assert.match(resolve, /tag_sha=\$\(git rev-parse --verify "refs\/tags\/v\$\{RELEASE_VERSION\}\^\{commit\}"\)/)
})

test('npm publishing runs only after the GitHub release exists', () => {
  const job = release.slice(release.indexOf('\n  publish-npm:'))
  assert.ok(job.length > 0, 'release.yml has no publish-npm job')
  assert.match(job, /needs: \[resolve, build, publish\]/)
  assert.match(job, /!cancelled\(\)/)
  assert.match(job, /inputs\.publish_npm_only \|\| needs\.publish\.result == 'success'/)
  // A single script owns the platform-first ordering; the workflow must not reimplement it as a
  // sequence of steps that a later edit could reorder.
  assert.match(job, /node scripts\/release\/publish-npm-packages\.mjs/)
  assert.doesNotMatch(job, /npm publish/)
})

test('npm-only recovery verifies the existing release and skips release and crate creation', () => {
  assert.match(release, /publish_npm_only:\s*\n\s*description:[^\n]+\n\s*type: boolean\s*\n\s*default: false/)
  const resolve = release.slice(release.indexOf('\n  resolve:'), release.indexOf('\n  build:'))
  const publish = release.slice(release.indexOf('\n  publish:'), release.indexOf('\n  publish-crate:'))
  const crate = release.slice(release.indexOf('\n  publish-crate:'), release.indexOf('\n  publish-npm:'))
  const npm = release.slice(release.indexOf('\n  publish-npm:'))
  assert.match(resolve, /check-recovery-release\.mjs/)
  assert.match(publish, /!inputs\.publish_npm_only/)
  assert.match(crate, /!inputs\.publish_npm_only/)
  assert.match(npm, /gh release download/)
  assert.match(npm, /published-release/)
  assert.match(npm, /verify-npm-binaries\.mjs/)
})

test('crate publishing uses the exact release commit and crates.io OIDC after the GitHub release', () => {
  const start = release.indexOf('\n  publish-crate:')
  const end = release.indexOf('\n  publish-npm:', start)
  assert.ok(start > -1 && end > start, 'release.yml has no bounded crate publication job')
  const job = release.slice(start, end)
  assert.match(job, /needs: \[resolve, build, publish\]/)
  assert.match(job, /inputs\.publish == true/)
  assert.match(job, /vars\.PUBLISH_CRATES == 'true'/)
  assert.match(job, /environment: crates-io/)
  assert.match(job, /permissions:\s*\n\s*contents: read\s*\n\s*id-token: write/)
  assert.match(job, /rust-lang\/crates-io-auth-action@/)
  assert.match(job, /CARGO_REGISTRY_TOKEN: \$\{\{ steps\.crates-io-auth\.outputs\.token \}\}/)
  assert.match(job, /cargo publish --locked/)
  assert.doesNotMatch(job, /secrets\.|cargo login/)
})

test('publication checks the live tag immediately before release creation and pins its target', () => {
  const publish = release.slice(release.indexOf('\n  publish:'), release.indexOf('\n  publish-crate:'))
  const finalCheck = publish.indexOf('validate-release-context.mjs --remote')
  const releaseAction = publish.indexOf('softprops/action-gh-release@')
  assert.ok(finalCheck > -1 && releaseAction > finalCheck)
  assert.match(publish, /fetch-depth: 0/)
  assert.match(publish, /target_commitish: \$\{\{ github\.sha \}\}/)
  assert.match(publish.slice(publish.indexOf('Revalidate live tag'), releaseAction), /EVENT_SHA: \$\{\{ github\.sha \}\}/)
})

test('GitHub release publishes signed archives, checksums, and provenance bundle', () => {
  const publish = release.slice(release.indexOf('\n  publish:'), release.indexOf('\n  publish-crate:'))
  const sign = publish.indexOf('cosign sign-blob --yes --bundle')
  const attest = publish.indexOf('actions/attest-build-provenance@')
  const releaseAction = publish.indexOf('softprops/action-gh-release@')
  assert.match(publish, /environment: github-release/)
  assert.ok(sign > -1 && attest > sign && releaseAction > attest,
    'signing and attestation must precede release creation')
  assert.match(publish, /dist\/\*\.tar\.gz dist\/\*\.sha256/)
  assert.match(publish, /dist\/vitest-dedup\.intoto\.jsonl/)
  assert.match(publish.slice(releaseAction), /dist\/\*\.sigstore\.json/)
})

test('publication is gated by the exact release tag, not owner identity', () => {
  const publishStart = release.indexOf('\n  publish:')
  const crateStart = release.indexOf('\n  publish-crate:')
  const npmStart = release.indexOf('\n  publish-npm:')
  assert.ok(publishStart > -1 && crateStart > publishStart && npmStart > crateStart,
    'release.yml has no bounded publish jobs')

  const githubReleaseJob = release.slice(publishStart, crateStart)
  const crateReleaseJob = release.slice(crateStart, npmStart)
  const npmReleaseJob = release.slice(npmStart)
  assert.doesNotMatch(release, /github\.(?:triggering_actor|actor) == github\.repository_owner/)
  assert.match(githubReleaseJob, /if: inputs\.publish == true/)
  assert.match(crateReleaseJob, /inputs\.publish == true/)
  assert.match(npmReleaseJob, /inputs\.publish == true/)
  assert.match(npmReleaseJob, /vars\.PUBLISH_NPM == 'true'/)
})

test('manual dispatch rehearses by default and publishing requires an explicit choice', () => {
  assert.match(release, /workflow_dispatch:\s*\n\s*inputs:[\s\S]*?publish:\s*\n\s*description:[^\n]+\n\s*type: boolean\s*\n\s*default: false/)

  const rehearseStart = release.indexOf('\n  rehearse:')
  const publishStart = release.indexOf('\n  publish:')
  const npmStart = release.indexOf('\n  publish-npm:')
  assert.ok(rehearseStart > -1 && publishStart > rehearseStart && npmStart > publishStart)
  const rehearse = release.slice(rehearseStart, publishStart)
  const publish = release.slice(publishStart, release.indexOf('\n  publish-crate:'))
  assert.match(rehearse, /if: inputs\.publish != true/)
  assert.match(rehearse, /needs: \[resolve, build\]/)
  assert.match(rehearse, /verify-npm-binaries\.mjs/)
  assert.match(rehearse, /install-simulation\.test\.mjs/)
  assert.match(rehearse, /prepare-npm-packages\.mjs native/)
  assert.match(rehearse, /npm pack/)
  assert.match(rehearse, /actions\/upload-artifact@/)
  assert.doesNotMatch(rehearse, /action-gh-release|npm publish|publish-npm-packages\.mjs/)
  assert.match(publish, /if: inputs\.publish == true/)
  assert.match(release, /permissions:\s*\n\s*contents: read/)
  assert.match(publish, /permissions:\s*\n\s*contents: write\s*\n\s*id-token: write\s*\n\s*attestations: write/)
})

test('release builds install each target for the repository toolchain', () => {
  const start = release.indexOf('\n  build:')
  const end = release.indexOf('\n  rehearse:', start)
  assert.ok(start > -1 && end > start, 'release.yml has no bounded build job')
  const build = release.slice(start, end)
  const install = build.indexOf('rustup target add "$RUST_TARGET"')
  const compile = build.indexOf('cargo build --release --locked --target "$RUST_TARGET"')
  assert.ok(install > -1 && compile > install, 'target must be installed for the active toolchain before compilation')
})

test('npm publishing authenticates with OIDC and never a stored token', () => {
  const job = release.slice(release.indexOf('\n  publish-npm:'))
  assert.match(job, /permissions:\s*\n\s*contents: read\s*\n\s*id-token: write/)
  assert.match(job, /check-npm-trusted-publishing\.mjs/)
  // Matched against usage, not prose: the job's own comment explains why no token exists, and a
  // check that tripped over the explanation would have to be deleted rather than satisfied.
  assert.doesNotMatch(release, /NODE_AUTH_TOKEN:/)
  // Registry credentials stay OIDC-only. The one stored secret is the website dispatch token, a
  // cross-repository call that cannot use OIDC; any other secret reference fails.
  const secretNames = [...release.matchAll(/\$\{\{\s*secrets\.([A-Za-z0-9_]+)/g)].map((match) => match[1])
  assert.deepEqual(secretNames.filter((name) => name !== 'SITE_DISPATCH_TOKEN'), [])
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

test('platform packages use checked-in workspaces and gain binaries at release time', () => {
  assert.match(release, /prepare-npm-packages\.mjs native/)
  assert.deepEqual(rootManifest.workspaces, ['npm/platforms/*'])
  assert.match(ci, /npm ci --force --no-audit --no-fund/)
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
