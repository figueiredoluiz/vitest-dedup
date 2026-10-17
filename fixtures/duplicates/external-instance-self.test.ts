// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'external-instance-self';let d=new Date(0);d=new Date(d.getTime()+1);expect(d.getTime()).toBe(1);});
test('second',()=>{void 'external-instance-self';let d=new Date(0);d=new Date(d.getTime()+1);expect(d.getTime()).toBe(1);});
