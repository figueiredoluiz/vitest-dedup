import assert from 'node:assert/strict'
import test from 'node:test'

import { validateRecoveryRelease } from '../../scripts/release/check-recovery-release.mjs'

const sha = 'a'.repeat(40)
const release = { tagName: 'v0.2.0', targetCommitish: sha, isDraft: false }

test('npm recovery accepts only the published release for the exact dispatched commit', () => {
  assert.doesNotThrow(() => validateRecoveryRelease(release, 'v0.2.0', sha))
  for (const mismatch of [
    { tagName: 'v0.1.0' },
    { targetCommitish: 'b'.repeat(40) },
    { isDraft: true },
  ]) {
    assert.throws(() => validateRecoveryRelease({ ...release, ...mismatch }, 'v0.2.0', sha),
      /matching published GitHub Release/)
  }
})
