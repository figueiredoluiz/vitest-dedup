// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'instance-binding-renamed';class C{};const a=new C();const b=new C();expect(a).toBe(a);});
test('second',()=>{void 'instance-binding-renamed';class Renamed{};const first=new Renamed();const second=new Renamed();expect(first).toBe(first);});
