import {test,expect} from 'vitest'
export function make(){const id=fn=>fn;return {
async:()=>id(test)('async',async()=>{await Promise.resolve();expect(2+3).toBe(5)}),
sync:()=>id(test)('sync',()=>expect(2+3).toBe(5))
}}
