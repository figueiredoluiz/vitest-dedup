// @vitest-dedup:extracted 2
// @vitest-dedup:not-runnable The first test reads the separately allocated array through a closure and fails.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L8 "first"
// @vitest-dedup:participant L9 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-callable-field';const a=[];const b=[];class C{read=()=>a}const c=new C();expect(c.read()).toBe(b);});
test('second',()=>{void 'capture-binding-callable-field';const a=[];const b=[];class C{read=()=>b}const c=new C();expect(c.read()).toBe(b);});
