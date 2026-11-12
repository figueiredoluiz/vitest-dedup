import {test,expect} from 'vitest'
declare enum Missing { Value }
/** Read the local erased declaration before constructing a leaf. */
function make(){const unused=Missing;return {run:()=>test('local',()=>expect(1).toBe(1))}}
const runner=make()
runner.run()
runner.run()
