// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The first test observes the separately allocated captured array and fails.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L8 "first"
// @vitest-dedup:participant L9 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-nested-static';const a=[];const b=[];class Outer{constructor(){class Inner{static value=a}globalThis.captureObserved=Inner.value}}new Outer();expect(globalThis.captureObserved).toBe(b);});
test('second',()=>{void 'capture-binding-nested-static';const a=[];const b=[];class Outer{constructor(){class Inner{static value=b}globalThis.captureObserved=Inner.value}}new Outer();expect(globalThis.captureObserved).toBe(b);});
