globalThis.vi = { doMock() {} }
vi.doMock('./dep')

test('first rewritten mock global', () => {
  expect(1).toBe(1)
})

test('second rewritten mock global', () => {
  expect(1).toBe(1)
})
