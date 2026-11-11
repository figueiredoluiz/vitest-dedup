// Source-keyed inline snapshots cannot share a test.each assertion site.
// @vitest-dedup:extracted 14
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The rule refuses source-site-sensitive assertions; the stream has no view of the proposed shared assertion site.
// @vitest-dedup:participant L33 "snapshot a"
// @vitest-dedup:participant L34 "snapshot b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The rule refuses source-site-sensitive assertions; the stream has no view of the proposed shared assertion site.
// @vitest-dedup:participant L35 "soft snapshot a"
// @vitest-dedup:participant L36 "soft snapshot b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The rule refuses source-site-sensitive assertions; the stream has no view of the proposed shared assertion site.
// @vitest-dedup:participant L37 "computed snapshot a"
// @vitest-dedup:participant L38 "computed snapshot b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The rule refuses source-site-sensitive assertions; the stream has no view of the proposed shared assertion site.
// @vitest-dedup:participant L39 "throw snapshot a"
// @vitest-dedup:participant L40 "throw snapshot b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The rule refuses source-site-sensitive assertions; the stream has no view of the proposed shared assertion site.
// @vitest-dedup:participant L41 "concatenated snapshot a"
// @vitest-dedup:participant L42 "concatenated snapshot b"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L43 "ordinary a"
// @vitest-dedup:participant L44 "ordinary b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Both bodies reach a source-sensitive assertion through local helpers.
// @vitest-dedup:participant L52 "shared snapshot a"
// @vitest-dedup:participant L53 "shared snapshot b"

import { test, expect } from 'vitest'

test('snapshot a', () => { expect('a').toMatchInlineSnapshot(`"a"`) })
test('snapshot b', () => { expect('b').toMatchInlineSnapshot(`"b"`) })
test('soft snapshot a', () => { expect.soft('a').toMatchInlineSnapshot(`"a"`) })
test('soft snapshot b', () => { expect.soft('b').toMatchInlineSnapshot(`"b"`) })
test('computed snapshot a', () => { void 'computed-snapshot'; expect(3).toBe(3); expect('a')['toMatchInlineSnapshot'](`"a"`) })
test('computed snapshot b', () => { void 'computed-snapshot'; expect(3).toBe(3); expect('b')['toMatchInlineSnapshot'](`"b"`) })
test('throw snapshot a', () => { expect(() => { throw new Error('a') }).toThrowErrorMatchingInlineSnapshot(`[Error: a]`) })
test('throw snapshot b', () => { expect(() => { throw new Error('b') }).toThrowErrorMatchingInlineSnapshot(`[Error: b]`) })
test('concatenated snapshot a', () => { void 'concatenated-snapshot'; expect(5).toBe(5); expect('a')['toMatch' + 'InlineSnapshot'](`"a"`) })
test('concatenated snapshot b', () => { void 'concatenated-snapshot'; expect(5).toBe(5); expect('b')['toMatch' + 'InlineSnapshot'](`"b"`) })
test('ordinary a', () => { void 'snapshot-retention'; expect('a').toBe('a') })
test('ordinary b', () => { void 'snapshot-retention'; expect('b').toBe('b') })

const checkSnapshot = () => assertSnapshot()
const assertSnapshot = () => {
  expect(checkSnapshot).toBeTypeOf('function')
  expect('shared').toMatchInlineSnapshot(`"shared"`)
}

test('shared snapshot a', () => { checkSnapshot(); expect('shared-a').toBe('shared-a') })
test('shared snapshot b', () => { checkSnapshot(); expect('shared-b').toBe('shared-b') })
