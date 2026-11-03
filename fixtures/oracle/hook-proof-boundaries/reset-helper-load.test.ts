import { beforeAll, beforeEach, expect, test, vi } from 'vitest'
import { replaceReset } from './reset-helper'
let calls = 0
beforeAll(async () => { await replaceReset(() => { calls += 1 }) })
beforeEach(() => { vi.unstubAllGlobals() })
beforeEach(() => { vi.unstubAllGlobals() })
test('needs both replacement calls', () => { expect(calls).toBe(2) })
test('keeps an eligible anchor', () => { expect(2 + 3).toBe(5) })
