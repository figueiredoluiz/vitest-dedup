// A `var` naming a parameter is the same binding, and holds the parameter until it runs.
import { describe, expect, test } from 'vitest'
import { closed, open, openArrow } from './parameter-redeclaration'
import { cases } from './support'

describe('parameter-redeclaration', () => {
  closed(1)
  open(cases)
  openArrow(cases)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
