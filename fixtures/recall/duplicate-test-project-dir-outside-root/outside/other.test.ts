// Outside the analysis root of `../root`; not part of this case's corpus.
import { expect, test } from 'vitest'

test('never analyzed', () => {
  expect(true).toBe(true)
})
