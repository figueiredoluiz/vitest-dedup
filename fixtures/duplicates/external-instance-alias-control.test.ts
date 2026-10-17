// @vitest-dedup:extracted 6
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable Erased wrappers do not turn owned class aliases into external constructors.
// @vitest-dedup:participant L14 "first"
// @vitest-dedup:participant L15 "second"
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable Parenthesized owned aliases retain conditional-write uncertainty.
// @vitest-dedup:participant L16 "parenthesized first"
// @vitest-dedup:participant L17 "parenthesized second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L18 "control first"
// @vitest-dedup:participant L19 "control second"
import {test,expect} from 'vitest';
test('first',()=>{void 'owned erased alias';class C{value=1};const K=C as any;let d=new K();if(true)d=new K();expect(d.value).toBe(1)});
test('second',()=>{void 'owned erased alias';class C{value=1};const K=C as any;let d=new K();if(true)d=new K();expect(d.value).toBe(1)});
test('parenthesized first',()=>{void 'owned parenthesized alias';class C{value=1};const K=(C);let d=new K();if(true)d=new K();expect(d.value).toBe(1)});
test('parenthesized second',()=>{void 'owned parenthesized alias';class C{value=1};const K=(C);let d=new K();if(true)d=new K();expect(d.value).toBe(1)});
test('control first',()=>{void 'stable wrapped alias';class C{value=1};const K=C as any;const d=new K();expect(d.value).toBe(1)});
test('control second',()=>{void 'stable wrapped alias';class Renamed{value=1};const Alias=Renamed satisfies any;const result=new Alias();expect(result.value).toBe(1)});
