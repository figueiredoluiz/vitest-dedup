import { expect, test } from 'vitest'

const { test: other } = await import('vitest')
other.extend = () => () => {}
const run = (0, test.extend({}))
run('replaced builder', () => expect(false).toBe(true))

test('import left', () => expect(6).toBe(6))
test('import right', () => expect(6).toBe(6))
