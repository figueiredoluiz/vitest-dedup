// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The first test intentionally observes the untouched array and fails.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L8 "first"
// @vitest-dedup:participant L9 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-constructor';const a=[];const b=[];class C{constructor(){a.push(1)}}new C();expect(b).toEqual([1]);});
test('second',()=>{void 'capture-binding-constructor';const a=[];const b=[];class C{constructor(){b.push(1)}}new C();expect(b).toEqual([1]);});
