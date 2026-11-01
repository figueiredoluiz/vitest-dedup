// Source-sensitive custom and destructured matchers retain their separate assertion sites.
// @vitest-dedup:extracted 6
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Source-site-sensitive assertions cannot share the proposed table call site.
// @vitest-dedup:participant L23 "custom a"
// @vitest-dedup:participant L24 "custom b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable An unknown computed destructuring key may select an inline snapshot matcher.
// @vitest-dedup:participant L25 "destructured a"
// @vitest-dedup:participant L26 "destructured b"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L27 "retain a"
// @vitest-dedup:participant L28 "retain b"

import { test, expect } from 'vitest'
import { Snapshots } from 'vitest'
expect.extend({
  toMatchTrimmedInlineSnapshot(received, snapshot) {
    return Snapshots.toMatchInlineSnapshot.call(this, received.trim(), snapshot)
  },
})
const matcher = ['to', 'Match', 'Inline', 'Snapshot'].join('')
test('custom a', () => { expect(' a ').toMatchTrimmedInlineSnapshot(`"a"`) })
test('custom b', () => { expect(' b ').toMatchTrimmedInlineSnapshot(`"b"`) })
test('destructured a', () => { const assertion = expect('a'); const { [matcher]: snap } = assertion; snap.call(assertion, `"a"`) })
test('destructured b', () => { const assertion = expect('b'); const { [matcher]: snap } = assertion; snap.call(assertion, `"b"`) })
test('retain a', () => { void 'custom-retention'; expect('a').toBe('a') })
test('retain b', () => { void 'custom-retention'; expect('b').toBe('b') })
