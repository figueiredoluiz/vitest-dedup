// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'external-instance-reflection';let d=new Date(0);if(true)d=new Date(1);expect(d.constructor.name).toBe('Date');});
test('second',()=>{void 'external-instance-reflection';let d=new Date(0);if(true)d=new Date(1);expect(d.constructor.name).toBe('Date');});
