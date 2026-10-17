// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable Conditional writes to owned local instances remain visibly unresolved.
// @vitest-dedup:participant L10 "first"
// @vitest-dedup:participant L11 "second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L12 "control first"
// @vitest-dedup:participant L13 "control second"
import {test,expect} from 'vitest';
test('first',()=>{void 'owned instance conditional';class C{value=1};let d=new C();if(true)d=new C();expect(d.value).toBe(1)});
test('second',()=>{void 'owned instance conditional';class C{value=1};let d=new C();if(true)d=new C();expect(d.value).toBe(1)});
test('control first',()=>{void 'external instance control';expect(Array.of(17)).toEqual([17])});
test('control second',()=>{void 'external instance control';expect(Array.of(17)).toEqual([17])});
