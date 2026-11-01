// Subsumption requires compatible fixture and table input identities.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable strict-subsequence candidates necessarily have unequal bodies; the owned Rust control changes only pro to free and requires one subsumption finding
// @vitest-dedup:participant L9 "a"
// @vitest-dedup:participant L10 "b"
import { test, expect } from 'vitest'
const free = test.extend({ quota: 100 }); const pro = test.extend({ quota: 100000 })
free('a', ({ quota }) => { expect(quota).toBeGreaterThan(0) })
pro('b', ({ quota }) => { expect(quota).toBeGreaterThan(0); expect(quota).toBeLessThan(200000) })
