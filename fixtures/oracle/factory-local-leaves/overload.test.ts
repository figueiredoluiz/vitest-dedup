import {test,expect} from 'vitest'
function make(value: number): {read:()=>void}
function make(value: number){return {read:()=>test('case',()=>expect(value).toBe(1))}}
const a=make(1)
a.read()
const b=make(1)
b.read()
const c=make(2)
c.read()
