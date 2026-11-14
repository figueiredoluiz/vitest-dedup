import {test,expect} from 'vitest'
/** Register a small scalar-bound case. */
export function run(value){test(`case ${value}`,()=>expect(value).toBe(value))}
