import assert from 'node:assert/strict'
import test from 'node:test'

import { validateRecoveryRelease } from '../../scripts/release/check-recovery-release.mjs'

const release = { tagName: 'v0.2.0', targetCommitish: 'main', isDraft: false }

test('npm recovery accepts a published release for the live-validated tag', () => {
  // GitHub ignores targetCommitish when the tag already exists. The live-tag guard checks the SHA.
  assert.doesNotThrow(() => validateRecoveryRelease(release, 'v0.2.0'))
  for (const mismatch of [
    { tagName: 'v0.1.0' },
    { isDraft: true },
  ]) {
    assert.throws(() => validateRecoveryRelease({ ...release, ...mismatch }, 'v0.2.0'),
      /matching published GitHub Release/)
  }
  assert.throws(() => validateRecoveryRelease(undefined, 'v0.2.0'), /matching published/)
})
