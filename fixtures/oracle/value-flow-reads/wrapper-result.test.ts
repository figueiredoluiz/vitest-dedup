import { expect, test } from 'vitest'
/** Registers `own` and returns a result object. */ function run() { test('own', () => {}); return { stderr: '' } }
const { stderr } = run()
test('reads the result', () => { expect(stderr).toBe('') })
