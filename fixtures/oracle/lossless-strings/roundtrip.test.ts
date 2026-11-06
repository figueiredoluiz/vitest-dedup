import { expect, test } from 'vitest'

test.each(['\uD800'])('surrogate row', value => { expect(value).toBe('\uD800') })
test('surrogate literal', () => { expect('\uD800').toBe('\uD800') })
test.each(['\uFFFDd800'])('replacement row', value => { expect(value).toBe('\uFFFDd800') })
test('replacement literal', () => { expect('\uFFFDd800').toBe('\uFFFDd800') })
test.each(['a\n\tb\r\b\f"\\'])('escaped %j', value => { expect(value).toBeTruthy() })
