import { expect, test, vi } from 'vitest'

vi.spyOn(String.prototype, 'replaceAll').mockImplementation(() => 'runtime title')

let patched = 'sentinel'
patched = patched.replaceAll('sentinel', 'static title')
test(patched, () => { expect(patched).toBe('runtime title') })
