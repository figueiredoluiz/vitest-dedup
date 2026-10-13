import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { validateReleaseContext } from '../../scripts/release/validate-release-context.mjs'

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
  const run = (overrides) => spawnSync(process.execPath, [script], {
    env: { ...env, ...overrides }, encoding: 'utf8',
  })
  assert.equal(run({}).status, 0)
  const branch = run({ EVENT_REF: 'refs/heads/main' })
  assert.equal(branch.status, 1)
  assert.match(branch.stderr, /matching release tag/)
})
