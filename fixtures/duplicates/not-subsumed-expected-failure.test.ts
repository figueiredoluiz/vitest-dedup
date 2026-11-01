// A passing larger test proves a smaller test's events only when both expect to pass. Each pair
// has its own subject, so it can only pair with itself.
// @vitest-dedup:extracted 10
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable strict-subsequence candidates necessarily have unequal bodies; the owned Rust control removes only the failure markers and requires a finding per pair
// @vitest-dedup:pending the default engine compares expected failures for subsumption; the next engine refuses them
// @vitest-dedup:participant L31 "reads the total"
// @vitest-dedup:participant L32 "reads the total, then a wrong tax"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable strict-subsequence candidates necessarily have unequal bodies; the owned Rust control removes only the failure markers and requires a finding per pair
// @vitest-dedup:pending the default engine compares expected failures for subsumption; the next engine refuses them
// @vitest-dedup:participant L33 "reads the sku"
// @vitest-dedup:participant L34 "reads the sku, then a wrong size"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable strict-subsequence candidates necessarily have unequal bodies; the owned Rust control removes only the failure markers and requires a finding per pair
// @vitest-dedup:pending the default engine compares expected failures for subsumption; the next engine refuses them
// @vitest-dedup:participant L35 "misreads the first ticket"
// @vitest-dedup:participant L36 "reads two tickets"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable strict-subsequence candidates necessarily have unequal bodies; the owned Rust control removes only the failure markers and requires a finding per pair
// @vitest-dedup:pending the default engine compares expected failures for subsumption; the next engine refuses them
// @vitest-dedup:participant L37 "misreads the name"
// @vitest-dedup:participant L38 "misreads the name, then the city"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable strict-subsequence candidates necessarily have unequal bodies; the owned Rust control removes only the failure markers and requires a finding per pair
// @vitest-dedup:pending the default engine compares expected failures for subsumption; the next engine refuses them
// @vitest-dedup:participant L40 "inverted suite > misreads the zone"
// @vitest-dedup:participant L41 "inverted suite > misreads the zone, then the code"
import { describe, expect, test } from 'vitest'
const order = { total: 40, tax: 4, sku: 'a1', size: 2, name: 'ana', city: 'rio', zone: 9, code: 7 }; let ticket = 0; const nextTicket = () => ++ticket
test('reads the total', () => { expect(order.total).toBe(40) })
test.fails('reads the total, then a wrong tax', () => { expect(order.total).toBe(40); expect(order.tax).toBe(5) })
test('reads the sku', () => { expect(order.sku).toBe('a1') })
test('reads the sku, then a wrong size', { fails: true }, () => { expect(order.sku).toBe('a1'); expect(order.size).toBe(3) })
test.fails('misreads the first ticket', () => { expect(nextTicket()).toBe(2) })
test('reads two tickets', () => { expect(nextTicket()).toBe(2); expect(nextTicket()).toBe(3) })
test.fails('misreads the name', () => { expect(order.name).toBe('bia') })
test.fails('misreads the name, then the city', () => { expect(order.name).toBe('bia'); expect(order.city).toBe('rio') })
describe('inverted suite', { fails: true }, () => {
  test('misreads the zone', () => { expect(order.zone).toBe(1) })
  test('misreads the zone, then the code', () => { expect(order.zone).toBe(1); expect(order.code).toBe(7) })
})
