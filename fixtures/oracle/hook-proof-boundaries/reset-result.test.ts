import { beforeEach, expect, test, vi as tools } from 'vitest'
let calls = 0
beforeEach(() => { tools.clearAllMocks() })
beforeEach(() => { tools.clearAllMocks() })
test('replaces through a returned alias', () => {
  const alias = tools.clearAllMocks()
  alias.clearAllMocks = () => { calls += 1; return alias }
})
test('needs both replacement calls', () => { expect(calls).toBe(3) })
test('keeps an eligible anchor', () => { expect(2 + 3).toBe(5) })
