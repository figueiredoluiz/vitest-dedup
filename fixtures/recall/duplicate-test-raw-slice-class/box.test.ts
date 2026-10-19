// RECALL positive -- identical bodies stay a duplicate when a raw-text slice names a class
// instance declared in the body.
//
// `[box.value] = [1]` is a destructuring assignment target, which the serializer keeps as source
// text. It names `box`, an instance of a class declared in the same body, and both tests are
// otherwise identical. The binding is tied to its slot and the class is rendered where it is
// declared, so the pair is compared and reported; routing the name through the class-expanding
// path instead marks both bodies unresolved and the finding disappears.
//
// Named weakening: give the second class a different field initializer and the pair is no longer a
// duplicate.
import { expect, test } from 'vitest'

test('assigns through a destructuring target on a class instance', () => {
  class Box {
    value = 0
  }
  const box = new Box()
  ;[box.value] = [1]
  expect(box.value).toBe(1)
})

test('assigns through a destructuring target on a class instance again', () => {
  class Box {
    value = 0
  }
  const box = new Box()
  ;[box.value] = [1]
  expect(box.value).toBe(1)
})
