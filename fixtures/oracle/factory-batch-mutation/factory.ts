import {test,expect} from 'vitest';let state=0;const id=fn=>fn;export function make(){return {cases:{write:()=>id(test)('write',()=>{state=1}),read:()=>id(test)('read',()=>expect(state).toBe(0))}}}
