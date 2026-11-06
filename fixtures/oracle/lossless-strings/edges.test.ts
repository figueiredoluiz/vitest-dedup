import { expect, test } from 'vitest'

test.each([
  { value: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\uD800bb' },
  { value: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\uDC00bb' },
  { value: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\uD83D\uDE00b' },
  { value: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa�bb' },
])('edge $value', value => { expect(value).toBeTruthy() })
