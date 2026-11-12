import {test,expect} from 'vitest'
let state=0
function make(){const id=fn=>fn;return {write:()=>id(test)('write',()=>{state=1})}}
const r=make()
r.write()
test('reader a',()=>expect(state).toBe(0))
test('reader b',()=>expect(state).toBe(0))
test('independent a',()=>expect(2+3).toBe(5))
test('independent b',()=>expect(2+3).toBe(5))
