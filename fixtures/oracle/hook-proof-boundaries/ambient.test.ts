import { beforeEach, expect, test } from 'vitest'
declare let hookBoundaryState: number
beforeEach(() => { hookBoundaryState = 1 })
beforeEach(() => { hookBoundaryState = 1 })
test('requires both global setter calls', () => {
  expect(globalThis.hookBoundaryWrites).toBe(2)
})
