import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { remoteTagCommit, validateReleaseContext } from '../../scripts/release/validate-release-context.mjs'

const sha = 'a'.repeat(40)
const otherSha = 'b'.repeat(40)
const tagged = {
  publish: true,
  tag: 'v0.2.0',
  ref: 'refs/tags/v0.2.0',
  eventSha: sha,
  releaseSha: sha,
  tagSha: sha,
}

test('live tag lookup accepts only the exact lightweight or peeled annotated commit', () => {
  const ref = 'refs/tags/v0.2.0'
  assert.equal(remoteTagCommit(`${sha}\t${ref}\n`, 'v0.2.0'), sha)
  assert.equal(remoteTagCommit(`${otherSha}\t${ref}\n${sha}\t${ref}^{}\n`, 'v0.2.0'), sha)
  assert.equal(remoteTagCommit(`${otherSha}\t${ref}\n`, 'v0.2.0'), otherSha)
  assert.throws(() => remoteTagCommit('', 'v0.2.0'), /release tag must exist/)
  assert.throws(() => remoteTagCommit(`${sha}\trefs/tags/v0.2.0-rc.1\n`, 'v0.2.0'), /release tag must exist/)
})

test('publication accepts an existing exact tag at the event commit', () => {
  assert.doesNotThrow(() => validateReleaseContext(tagged))
})

test('branch and different-tag dispatches cannot publish', () => {
  for (const ref of ['refs/heads/main', 'refs/tags/v0.1.0', 'refs/tags/v0.2.0-rc.1']) {
    assert.throws(() => validateReleaseContext({ ...tagged, ref }), /matching release tag/)
  }
})

test('missing, moved, and conflicting tag commits cannot publish', () => {
  for (const tagSha of ['', otherSha, 'not-a-sha']) {
    assert.throws(() => validateReleaseContext({ ...tagged, tagSha }), /tag must exist/)
  }
})

test('checkout and workflow event must match even for rehearsals', () => {
  assert.throws(() => validateReleaseContext({ ...tagged, releaseSha: otherSha }), /checked-out commit/)
  assert.throws(() => validateReleaseContext({ ...tagged, eventSha: '' }), /checked-out commit/)
  assert.doesNotThrow(() => validateReleaseContext({ ...tagged, publish: false, ref: 'refs/heads/main', tagSha: '' }))
})

test('workflow environment adapter accepts the exact tag and rejects a branch', () => {
  const env = {
    ...process.env,
    PUBLISH_RELEASE: 'true',
    RELEASE_TAG: tagged.tag,
    EVENT_REF: tagged.ref,
    EVENT_SHA: tagged.eventSha,
    RELEASE_SHA: tagged.releaseSha,
    TAG_SHA: tagged.tagSha,
  }
  const script = fileURLToPath(new URL('../../scripts/release/validate-release-context.mjs', import.meta.url))
  /** Runs the release-context CLI with one environment variation. */
  const run = (overrides) => spawnSync(process.execPath, [script], {
    env: { ...env, ...overrides }, encoding: 'utf8',
  })
  assert.equal(run({}).status, 0)
  const branch = run({ EVENT_REF: 'refs/heads/main' })
  assert.equal(branch.status, 1)
  assert.match(branch.stderr, /matching release tag/)
})

test('publication rechecks a live remote tag and rejects movement or deletion', () => {
  const root = mkdtempSync(join(tmpdir(), 'vitest-dedup-release-tag-'))
  const repo = join(root, 'source')
  const origin = join(root, 'origin.git')
  /** Executes Git only inside the disposable release-test repositories. */
  const git = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: 'pipe' }).trim()
  try {
    git(root, 'init', '--bare', '-q', origin)
    git(root, 'init', '-q', repo)
    git(repo, 'config', 'user.name', 'Release Test')
    git(repo, 'config', 'user.email', 'release-test@example.invalid')
    git(repo, 'commit', '--allow-empty', '-qm', 'release commit')
    const releaseSha = git(repo, 'rev-parse', 'HEAD')
    git(repo, 'remote', 'add', 'origin', origin)
    git(repo, 'tag', '-a', 'v0.2.0', '-m', 'release')
    git(repo, 'push', '-q', 'origin', 'refs/tags/v0.2.0')
    const script = fileURLToPath(new URL('../../scripts/release/validate-release-context.mjs', import.meta.url))
    const env = { ...process.env, PUBLISH_RELEASE: 'true', RELEASE_TAG: 'v0.2.0',
      EVENT_REF: 'refs/tags/v0.2.0', EVENT_SHA: releaseSha, RELEASE_SHA: releaseSha }
    /** Invokes the same live-tag check as the publish job against the disposable origin. */
    const run = () => spawnSync(process.execPath, [script, '--remote'], { cwd: repo, env, encoding: 'utf8' })
    assert.equal(run().status, 0)

    git(repo, 'commit', '--allow-empty', '-qm', 'different commit')
    git(repo, 'tag', '-f', 'v0.2.0')
    git(repo, 'push', '-q', '--force', 'origin', 'refs/tags/v0.2.0')
    assert.match(run().stderr, /release tag must exist at the checked-out commit/)
    git(repo, 'push', '-q', 'origin', ':refs/tags/v0.2.0')
    assert.notEqual(run().status, 0)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
