import { beforeAll, beforeEach, expect, test, vi } from 'vitest'
let calls = 0
beforeAll(() => { const spy = vi.fn(); spy(); spy.mockClear = () => { calls += 1; return spy } })
beforeEach(() => { vi.clearAllMocks() })
beforeEach(() => { vi.clearAllMocks() })
test('needs both replacement calls', () => { expect(calls).toBe(3) })
test('keeps an eligible anchor', () => { expect(2 + 3).toBe(5) })
