import { beforeAll, beforeEach, expect, test, vi } from 'vitest'
let calls = 0
beforeAll(() => { const other = vi.valueOf(); other.unstubAllGlobals = () => { calls += 1; return other } })
beforeEach(() => { vi.unstubAllGlobals() })
beforeEach(() => { vi.unstubAllGlobals() })
test('needs both replacement calls', () => { expect(calls).toBe(2) })
test('keeps an eligible anchor', () => { expect(2 + 3).toBe(5) })
