import { describe, expect, test } from 'vitest'
import { first, '�d800' as second, '�fffd' as third } from './values�fffd'

test('\uD800', () => { expect(1).toBe(1) })
test('\uFFFDd800', () => { expect(2).toBe(2) })
test('\uDC00', () => { expect(3).toBe(3) })
test('\uFFFDdc00', () => { expect(4).toBe(4) })
test(`\uD800`, () => { expect(5).toBe(5) })
test(`\uFFFDd800`, () => { expect(6).toBe(6) })
test(`\uD83D${'\uDE00'}`, () => { expect(7).toBe(7) })
test('😀', () => { expect(8).toBe(8) })
test.each(['\uD800', '\uFFFDd800'])('s %s', value => { expect(value).toBeTruthy() })
test.each(['\uD800', '\uFFFDd800'])('j %j', value => { expect(value).toBeTruthy() })
test.each([{ value: '\uD800' }, { value: '\uFFFDd800' }])('$value', value => {
  expect(value).toBeTruthy()
})
describe('\uD800', () => {
  test('\uFFFDd800', () => { expect(9).toBe(9) })
})
test.each([first, first, second, third])('imported %s', value => { expect(value).toBeTruthy() })
