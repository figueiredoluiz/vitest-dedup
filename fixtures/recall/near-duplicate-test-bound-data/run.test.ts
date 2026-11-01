import { expect, test } from 'vitest'

const send = (value: unknown) => value
const receive = (value: unknown) => value

test('passes one value through', () => {
  const result = { values: [{ index: 0, value: 1 }], error: null }
  send(result)
  expect(receive(result)).toBe(result)
})

test('passes an error through', () => {
  const result = { values: null, error: { type: 'parse' } }
  send(result)
  expect(receive(result)).toBe(result)
})
