import { expect, test as control, vi } from 'vitest'

vi.stubGlobal('test', { extend: () => () => {} })
const run = (0, test.extend({}))
run('replaced global builder', () => expect(false).toBe(true))

control('global stub left', () => expect(9).toBe(9))
control('global stub right', () => expect(9).toBe(9))
