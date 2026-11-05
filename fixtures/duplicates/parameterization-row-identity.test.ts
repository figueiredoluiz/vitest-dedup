// Repeated composite uses retain their row when allocation identity or mutation is observable.
// @vitest-dedup:extracted 6
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The callback observes one shared cell through multiple reads.
// @vitest-dedup:participant L17 "identity first"
// @vitest-dedup:participant L18 "identity second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A write prevents readonly substitution of the shared cell.
// @vitest-dedup:participant L20 "write first"
// @vitest-dedup:participant L21 "write second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Reading an object-valued member retains allocation identity.
// @vitest-dedup:participant L23 "nested first"
// @vitest-dedup:participant L24 "nested second"
import { test, expect } from 'vitest'

test.each([{ label: 'first', value: {} }])('identity first', ({ value }) => expect(value === value).toBe(true))
test.each([{ label: 'second', value: {} }])('identity second', ({ value }) => expect(value === value).toBe(true))

test.each([{ label: 'first', value: { a: 1 } }])('write first', ({ value }) => { value.a++; expect(value.a).toBe(2) })
test.each([{ label: 'second', value: { a: 1 } }])('write second', ({ value }) => { value.a++; expect(value.a).toBe(2) })

test.each([{ label: 'first', value: { nested: {} } }])('nested first', ({ value }) => expect(value.nested === value.nested).toBe(true))
test.each([{ label: 'second', value: { nested: {} } }])('nested second', ({ value }) => expect(value.nested === value.nested).toBe(true))
