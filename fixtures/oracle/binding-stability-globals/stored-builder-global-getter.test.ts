import { expect, test as control } from 'vitest'

globalThis.__defineGetter__('test', () => ({ extend: () => () => {} }))
const run = (0, test.extend({}))
run('replaced global getter builder', () => expect(false).toBe(true))

control('global getter left', () => expect(14).toBe(14))
control('global getter right', () => expect(14).toBe(14))
