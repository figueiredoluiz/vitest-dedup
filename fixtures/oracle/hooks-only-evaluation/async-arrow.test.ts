// An async arrow's implicit return is resolved too.
import { describe, expect, test } from 'vitest'
import { arrowClosed, arrowOpen } from './helpers'; import { thenable } from './support'

describe('async-arrow', () => {
  arrowClosed(thenable)
  arrowOpen(thenable)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
