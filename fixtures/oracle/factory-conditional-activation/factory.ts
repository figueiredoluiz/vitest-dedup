import {test,expect} from 'vitest'
export function make(enabled){const run=fn=>fn.runIf(enabled);const skip=fn=>fn.skipIf(enabled);return {run:()=>run(test)('run case',()=>expect(1).toBe(1)),skip:()=>skip(test)('skip case',()=>expect(1).toBe(1))}}
