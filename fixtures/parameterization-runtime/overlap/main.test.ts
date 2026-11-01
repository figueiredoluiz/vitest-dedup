// Owned table-row order control; every declared case must remain present.
import { test, expect } from 'vitest'

test.each([1, 2])('first %i', value => { expect(value).toBe(value) })
test.each([1, 3])('second %i', value => { expect(value).toBe(value) })
