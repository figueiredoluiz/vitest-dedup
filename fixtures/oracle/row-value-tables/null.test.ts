import { describe, expect, test } from 'vitest'
import { NULL } from './null-values'

test.for([NULL])('whole %s', n => expect(n).toBeNull())
test('whole null', () => expect(0).toBe(0))
test.each([NULL])('spread %s', n => expect(n).toBeNull())
describe.for([NULL])('suite %s', n => {
  test('nested', () => expect(n).toBeNull())
})
