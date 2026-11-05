import { expect, test } from 'vitest'

test.for([[, 1]])('sparse %s %s', row => expect(row[0]).toBeUndefined())
test('sparse 1 1', () => expect(1).toBe(1))
