// Two tests running under `test.extend` fixtures bound to literals of DIFFERENT kinds: numeric
// 1000 versus string '1000'. Expected verdict: NOT a duplicate.
//
// Regression guard for the fourth instance of the same defect class in this codebase. A value's
// kind was erased and only its raw text retained, so `1000` and `'1000'` folded to the same string
// and two genuinely different fixture contexts compared equal — producing an error-severity
// finding on correct code.
//
// The earlier three were `vi.mock` factories, `test.extend` setup bodies, and literal
// serialization in the fingerprinter. This one hid in constant folding, which is shared with
// `.each` title rendering: titles need the RAW text (`<number:1000>` in a title is nonsense),
// comparison needs the TAGGED form. One function could not serve both.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates fixture
// @vitest-dedup:participant L31 "reads a numeric mode"
// @vitest-dedup:participant L35 "reads a string mode"
// CROSS-FILE COLLISION (declared, not dodged): the NUMERIC half below shares its body and its
// `test.extend` context with duplicates-fixture-same-literal.test.ts's first test, so
// `cross-file-duplicate` links those two. The string half is linked to nothing, across files or
// within one -- which is this fixture's own claim, restated across a file boundary for free.
// `fixtures/README.md`, "Incidental cross-file collisions", explains why it is declared.
// @vitest-dedup:expect rule=cross-file-duplicate verdict=redundant severity=warning
// @vitest-dedup:participant duplicates-fixture-same-literal.test.ts#L23 "reads the mode one way"
// @vitest-dedup:participant L31 "reads a numeric mode"
import { test, expect } from 'vitest'

const withNumber = test.extend({ mode: 1000 })
const withString = test.extend({ mode: '1000' })

withNumber('reads a numeric mode', ({ mode }) => {
  expect(mode).toBeDefined()
})

withString('reads a string mode', ({ mode }) => {
  expect(mode).toBeDefined()
})
