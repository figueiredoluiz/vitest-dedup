// Deep external identities must retain findings without exhausting the normalizer stack.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L12 "deep a"
// @vitest-dedup:participant L13 "deep b"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "positive a"
// @vitest-dedup:participant L15 "positive b"

import { test, expect } from 'vitest'
const deep = !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!1
test('deep a', () => { expect(deep).toBe(true) })
test('deep b', () => { expect(deep).toBe(true) })
test('positive a', () => { expect(1 + 1).toBe(2) })
test('positive b', () => { expect(1 + 1).toBe(2) })
