// RECALL control -- duplicate-test absent, shape: a `const` helper reached from a later `switch`
// case through a function declared in the helper's own case.
//
// Entering at `'2'` calls `run()`, which reads `declare` while its case's declarator has not run,
// so Vitest fails the file. The inner call is written after the declarator and in its case, but it
// executes from case `'2'`; the call is refused and the file is set aside.
import { test, expect } from 'vitest'

const mode = String(2)

switch (mode) {
  case '1':
    const declare = () => {
      test('hidden', () => {
        expect(1).toBe(1)
      })
    }
    /** Calls `declare`, from whichever case runs it. */
    function run() {
      declare()
    }
    break
  case '2':
    run()
}

test('visible', () => {
  expect(1).toBe(1)
})
