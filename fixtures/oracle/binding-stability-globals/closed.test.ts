// A hook read as a global is hooks-only.
import { closed } from './helpers'

describe('closed', () => {
  closed()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
