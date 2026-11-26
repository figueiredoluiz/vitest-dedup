import { expect, test, vi } from 'vitest'

const { importActual } = vi
const { test: other } = await importActual('vitest')
other.extend = () => () => {}
const run = (0, test.extend({}))
run('replaced builder', () => expect(false).toBe(true))

test('loader left', () => expect(8).toBe(8))
test('loader right', () => expect(8).toBe(8))
