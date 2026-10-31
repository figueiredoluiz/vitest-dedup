// RECALL positive -- duplicate-test expected, shape: a class the behavior trace cannot prove.
//
// The computed key leaves the trace incomplete, which says nothing about the flat form: these
// bodies are identical, and only a rule reading traces may decline them.
import { test } from 'vitest'

declare const key: string
/** Takes any value. */
declare function use(value: unknown): void

test('one', () => {
  class Keyed {
    [key]() {
      return 1
    }
  }
  use(new Keyed())
})

test('two', () => {
  class Keyed {
    [key]() {
      return 1
    }
  }
  use(new Keyed())
})
