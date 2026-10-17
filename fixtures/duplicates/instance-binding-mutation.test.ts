// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:not-runnable The second assertion intentionally fails to demonstrate identity.
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L8 "first"
// @vitest-dedup:participant L9 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'instance-binding-mutation';class C{value=0};const a=new C();const b=new C();b.value=1;expect(b.value).toBe(1);});
test('second',()=>{void 'instance-binding-mutation';class C{value=0};const a=new C();const b=new C();a.value=1;expect(b.value).toBe(1);});
