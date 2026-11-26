import { expect, test } from 'vitest'

const noop = () => {}
let register = (0, test.extend({}))
register = noop
register('reassigned', () => expect(7).toBe(7))

const stable = (0, test.extend({}))
{
  const stable = (0, noop)
  stable()
}
stable('stable binding', () => expect(8).toBe(8))
