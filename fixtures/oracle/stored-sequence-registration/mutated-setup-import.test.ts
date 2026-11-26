import { expect, test } from 'vitest'

await import('./setup-extension-replacement')
const run = (0, test.extend({}))
run('replaced setup builder', () => expect(false).toBe(true))

test('setup import left', () => expect(11).toBe(11))
test('setup import right', () => expect(11).toBe(11))
