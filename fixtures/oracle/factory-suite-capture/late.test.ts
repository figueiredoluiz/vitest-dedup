import {describe,test,expect} from 'vitest'
describe('s',()=>{
const r=make()
const value=1
function make(){const x=value;return {read:()=>test('t',()=>expect(x).toBe(1))}}
r.read()
})
