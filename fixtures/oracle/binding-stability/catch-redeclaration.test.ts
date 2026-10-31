// A `var` naming a catch binding is that binding while the block runs.
import { beforeEach, describe, expect, test } from 'vitest'
import { cases } from './support'

describe('catch-redeclaration', () => {
  try {
    throw cases
  } catch (hook: any) {
    hook(() => {})
    var hook: any = beforeEach
  }
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
