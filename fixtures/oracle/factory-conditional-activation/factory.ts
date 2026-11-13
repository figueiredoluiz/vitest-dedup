import {test,expect} from 'vitest'
/** Builds readers whose registrations depend on the captured activation flag. */ export function make(enabled){const run=fn=>fn.runIf(enabled);const skip=fn=>fn.skipIf(enabled);return {run:()=>run(test)('run case',()=>expect(1).toBe(1)),skip:()=>skip(test)('skip case',()=>expect(1).toBe(1))}}
