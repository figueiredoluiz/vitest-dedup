import { describe, test, expect } from 'vitest'

/** An arrow with a block body. */
export const arrowBlock = () => {
  test('arrow block', () => { expect(1).toBe(1) })
}

/** An arrow with a concise body, so the registration is its single expression. */
export const arrowConcise = () => describe('arrow concise', () => { test('inner', () => { expect(2).toBe(2) }) })

/** A function expression with a parameter read by its title. */
export const expression = function (label: string) {
  test(`expression ${label}`, () => { expect(3).toBe(3) })
}

/** A default-exported function declaration. */
export default function defaultDeclaration() {
  test('default declaration', () => { expect(4).toBe(4) })
}

/**
 * Register the aliased helper test used by the imported-runner runtime oracle.
 */
function aliased() {
  test('aliased', () => { expect(5).toBe(5) })
}
export { aliased as renamed }
