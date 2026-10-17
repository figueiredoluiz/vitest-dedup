// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'instance-binding-parameterized';class C{constructor(value){this.value=value}}const a=new C(1);const b=new C(1);expect(a.value).toBe(1);});
test('second',()=>{void 'instance-binding-parameterized';class C{constructor(value){this.value=value}}const a=new C(2);const b=new C(2);expect(a.value).toBe(2);});
