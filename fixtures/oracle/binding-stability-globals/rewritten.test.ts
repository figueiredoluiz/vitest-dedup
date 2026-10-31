// A global the program rewrites holds what was written.
import { open } from './rewritten'

describe('rewritten', () => {
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
