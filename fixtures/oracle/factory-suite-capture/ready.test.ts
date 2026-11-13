import {describe,test,expect} from 'vitest'
describe('s',()=>{
const value=1
const r=make()
function make(){const x=value;return {read:()=>test('t',()=>expect(x).toBe(1))}}
r.read()
test('retained',()=>expect(value).toBe(1))
})
