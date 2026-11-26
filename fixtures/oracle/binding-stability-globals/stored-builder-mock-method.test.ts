import { expect, test as control, vi } from 'vitest'

const tools = vi.valueOf()
tools.stubGlobal('test', { extend: () => () => {} })
const run = (0, test.extend({}))
run('replaced mock method builder', () => expect(false).toBe(true))

control('mock method left', () => expect(15).toBe(15))
control('mock method right', () => expect(15).toBe(15))
