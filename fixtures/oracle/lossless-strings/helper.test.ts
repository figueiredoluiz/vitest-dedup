import { expect, test } from 'vitest'

function register(label: string, value: string) {
  test(label, () => { expect(value).toBeTruthy() })
  test(`${label} copy`, () => { expect(value).toBeTruthy() })
}
register('first surrogate call', '\uD800')
register('replacement call', '\uFFFDd800')
register('second surrogate call', '\uD800')
