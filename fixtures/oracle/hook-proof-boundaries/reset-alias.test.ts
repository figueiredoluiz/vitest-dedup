import { beforeAll, beforeEach, expect, test, vi as tools, vitest as other } from 'vitest'
let calls = 0
beforeAll(() => { other.clearAllMocks = () => { calls += 1; return other } })
beforeEach(() => { tools.clearAllMocks() })
beforeEach(() => { tools.clearAllMocks() })
test('retains both calls after the runner reset', () => { expect(calls).toBe(3) })
test('keeps an eligible anchor', () => { expect(2 + 3).toBe(5) })
