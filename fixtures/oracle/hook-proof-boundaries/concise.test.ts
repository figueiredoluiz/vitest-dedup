import { beforeEach, describe, expect, test } from 'vitest'
let value = 0
describe('conditional registration in an expression body', () => [
  false ? beforeEach(() => { value = 1 }) : null,
  beforeEach(() => { value = 1 }),
  test('requires the only registered overwrite', () => { expect(value).toBe(1) }),
])
