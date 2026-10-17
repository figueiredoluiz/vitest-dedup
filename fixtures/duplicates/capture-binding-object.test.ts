// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The first test intentionally observes the untouched object and fails.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L8 "first"
// @vitest-dedup:participant L9 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-object';const a={};const b={};class C{constructor(){a.value=1}}new C();expect(b.value).toBe(1);});
test('second',()=>{void 'capture-binding-object';const a={};const b={};class C{constructor(){b.value=1}}new C();expect(b.value).toBe(1);});
