// Module identities remain exact; literal inputs to the same module may form a table.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier,literal
// @vitest-dedup:participant L12 "module a"
// @vitest-dedup:participant L13 "module b"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L14 "same module zero"
// @vitest-dedup:participant L15 "same module one"

import { test, expect } from 'vitest'
test('module a', async () => { const { value } = await import('./parameterization-module-a.js'); expect(value).toBe(1) })
test('module b', async () => { const { value } = await import('./parameterization-module-b.js'); expect(value).toBe(2) })
test('same module zero', async () => { const { add } = await import('./parameterization-module-a.js'); expect(add(0)).toBe(1) })
test('same module one', async () => { const { add } = await import('./parameterization-module-a.js'); expect(add(1)).toBe(2) })
