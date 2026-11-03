import { beforeAll, beforeEach, expect, test, vi } from 'vitest'
beforeAll(() => {
  let queued = Promise.resolve()
  for (let step = 0; step < 10; step += 1) queued = queued.then(() => {})
  void queued.then(() => { vi.stubGlobal('rc12Queued', 1) })
})
beforeEach(() => { vi.unstubAllGlobals() })
beforeEach(() => { vi.unstubAllGlobals() })
test('starts without the queued stub', () => { expect((globalThis as any).rc12Queued).toBeUndefined() })
test('keeps an eligible anchor', () => { expect(2 + 3).toBe(5) })
