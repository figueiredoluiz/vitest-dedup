import {test,expect} from 'vitest'
const a0=1
const a1=[a0,a0]
const a2=[a1,a1]
const a3=[a2,a2]
const a4=[a3,a3]
const a5=[a4,a4]
const a6=[a5,a5]
const a7=[a6,a6]
const a8=[a7,a7]
const a9=[a8,a8]
const a10=[a9,a9]
const a11=[a10,a10]
/** Register one test after constructing a shared inert graph. */
export function run(){test('bounded',()=>expect(1).toBe(1))}
