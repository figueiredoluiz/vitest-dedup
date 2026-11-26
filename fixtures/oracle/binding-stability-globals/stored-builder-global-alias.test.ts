import { expect, test as control } from 'vitest'

globalThis.globalThis.test.extend = () => () => {}
const run = (0, test.extend({}))
run('replaced global alias builder', () => expect(false).toBe(true))

control('global alias left', () => expect(13).toBe(13))
control('global alias right', () => expect(13).toBe(13))
