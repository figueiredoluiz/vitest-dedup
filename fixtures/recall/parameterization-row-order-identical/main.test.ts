// Reversing an identical table adds no literal variant, unlike existing positives; change the second table to [3, 4] to require a candidate.
import { test, expect } from 'vitest'

test.each([1, 2])('first table', value => { expect(value).toBe(value) })
test.each([2, 1])('second table', value => { expect(value).toBe(value) })
