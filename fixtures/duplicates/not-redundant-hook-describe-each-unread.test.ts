// Twin of redundant-hook-describe-each-lexical.test.ts with the rows read from a call: the
// table's population is unproven, so the callback's hooks have no known lifecycle chain and
// the pair must stay silent. Named weakening: give the table literal rows and the pair fires
// exactly as in the twin.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:participant L15 "beforeEach"
// @vitest-dedup:participant L18 "beforeEach"
// @vitest-dedup:isolates not-isolable the two hooks are byte-identical, so the isolated variable is the enclosing table's rows proof, which no participant fingerprint carries; the guard is the corpus gate's silence assertion plus the named weakening in the header
import { describe, test, expect, beforeEach } from 'vitest'

const rows = makeRows()

describe.each(rows())('row %s', () => {
  beforeEach(() => {
    value = 1
  })
  beforeEach(() => {
    value = 1
  })

  test('reads the value', () => {
    expect(value).toBe(1)
  })
})
