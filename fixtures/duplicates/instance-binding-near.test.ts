// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=near-duplicate-test verdict=similar-uncertain severity=warning
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'instance-binding-near';class C{};const a=new C();const b=new C();expect(a).toBe(a);});
test('second',()=>{void 'instance-binding-near';class C{};const a=new C();const b=new C();void 0;expect(a).toBe(a);});
