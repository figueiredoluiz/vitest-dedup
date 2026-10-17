// @vitest-dedup:extracted 4
// @vitest-dedup:not-runnable The first test intentionally observes the untouched array and fails.
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable A visible refusal of nested construction history, not an isolation claim between comparable bodies.
// @vitest-dedup:participant L11 "first"
// @vitest-dedup:participant L12 "second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L13 "control first"
// @vitest-dedup:participant L14 "control second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-nested';const a=[];const b=[];class C{constructor(){class Inner{constructor(){a.push(1)}}new Inner()}}new C();expect(b).toEqual([1]);});
test('second',()=>{void 'capture-binding-nested';const a=[];const b=[];class C{constructor(){class Inner{constructor(){b.push(1)}}new Inner()}}new C();expect(b).toEqual([1]);});
test('control first',()=>{void 'nested capture control';expect(Array.of(1)).toEqual([1]);});
test('control second',()=>{void 'nested capture control';expect(Array.of(1)).toEqual([1]);});
