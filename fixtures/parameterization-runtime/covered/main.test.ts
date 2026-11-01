// Owned table-row order control; every declared case must remain present.
import { test, expect } from 'vitest'

test.each([1, 2])('table %i', value => { expect(value).toBe(value) })
test('covered', () => { expect(2).toBe(2) })
