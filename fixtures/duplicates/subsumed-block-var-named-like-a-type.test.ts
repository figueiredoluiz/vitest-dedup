// A block `var` sharing its name with a type is still the variable, so the read is a local, as
// it is under a differently named type. Pins the parser's split of type and hoisted value bindings.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=subsumed-test verdict=similar-uncertain severity=warning
// @vitest-dedup:participant L8 "reads a block var named like a type"
// @vitest-dedup:participant L9 "counts up to the same rate"
import { expect, test } from 'vitest'
test('reads a block var named like a type', () => { { type Rate = number; var Rate = 7 } expect(Rate).toBe(7) })
test('counts up to the same rate', () => { let rate = 6; rate++; expect(rate).toBe(7) })
