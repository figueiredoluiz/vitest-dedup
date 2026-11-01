// A manual input absent from a table remains a candidate; ordinary manual-test positives do not exercise expanded rows.
import { test, expect } from 'vitest'

test.each([1, 2])('table', value => { expect(value).toBe(value) })
test('manual', () => { expect(3).toBe(3) })
