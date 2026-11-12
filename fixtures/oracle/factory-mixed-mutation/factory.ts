import {test,expect} from 'vitest';let state=0;const id=fn=>fn;export function make(){return {write:()=>id(test)('write',()=>{state=1})}}export function read(){test('read',()=>expect(state).toBe(0))}
