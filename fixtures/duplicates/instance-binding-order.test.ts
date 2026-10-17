// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:not-runnable The second assertion intentionally fails to demonstrate identity.
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L8 "first"
// @vitest-dedup:participant L9 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'instance-binding-order';globalThis.created=[];class C{constructor(){globalThis.created.push(this)}}const a=new C();const b=new C();globalThis.observed=a;expect(globalThis.observed).toBe(globalThis.created[0]);});
test('second',()=>{void 'instance-binding-order';globalThis.created=[];class C{constructor(){globalThis.created.push(this)}}const a=new C();const b=new C();globalThis.observed=b;expect(globalThis.observed).toBe(globalThis.created[0]);});
