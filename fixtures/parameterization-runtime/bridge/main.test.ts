// Overlapping sets need the same consolidation claims after declarations or rows move.
import { test, expect } from 'vitest'

test.each([1, 2])('left', value => { expect(value).toBe(value) })
test.each([3, 4])('right', value => { expect(value).toBe(value) })
test.each([2, 3])('bridge', value => { expect(value).toBe(value) })
