import { expect, test } from 'vitest'

test.__defineGetter__('extend', () => () => () => {})
const run = (0, test.extend({}))
run('replaced builder', () => expect(false).toBe(true))

test('getter left', () => expect(4).toBe(4))
test('getter right', () => expect(4).toBe(4))
