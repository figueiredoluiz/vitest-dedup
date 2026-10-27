// RECALL positive -- duplicate-test expected. A test in a helper initializer is extracted and compared with the visible test.
import { test, expect } from 'vitest'

const declare = () => {
  const registered = test('hidden', () => { expect(1).toBe(1) })
}
declare()
test('visible', () => { expect(1).toBe(1) })
