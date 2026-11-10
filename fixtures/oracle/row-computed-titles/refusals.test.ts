import { describe, expect, test } from 'vitest'

let closure = 'before'
/** Renames the closure title before it is read. */
function rename() { closure = 'after' }
rename()
test(closure, () => { expect(closure).toBe('after') })

let substituted = 'a-b'
substituted = substituted.replaceAll('-', '$&$&')
test(substituted, () => { expect(substituted).toBe('a--b') })

let empty = 'ab'
empty = empty.replaceAll('', '.')
test(empty, () => { expect(empty).toBe('.a.b.') })

let environment = 'local'
if (process.env.VITEST_DEDUP_UNSET) { environment = 'set' }
test(environment, () => { expect(environment).toBe('local') })

let compound = 'a'
compound += '!'
test(compound, () => { expect(compound).toBe('a!') })

let joined = 'a'
joined = joined + '-b'
test(joined, () => { expect(joined).toBe('a-b') })

let inline = 'a'
test.each([[inline = 'b']])(inline, (value) => { expect(value).toBe('b') })

describe.each([['x'], ['y']])('%s', (key) => {
  let switched = 'base'
  switch (key) { case 'y': switched = 'why' }
  test(switched, () => { expect(key).toBeTruthy() })
})

let held = 'held'
class Holder {
  static {
    test(held, () => { expect(Holder.name).toBe('Holder') })
  }
}

let restored = 'before'
const saved = restored
restored = 'after'
restored = saved
test(restored, () => { expect(restored).toBe('before') })
