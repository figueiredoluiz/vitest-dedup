// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-closure-renamed';const a=1;const helper=()=>a;class C{value=helper()}const result=new C();expect(result.value).toBe(1);});
test('second',()=>{void 'capture-binding-closure-renamed';const input=1;const read=()=>input;class Renamed{value=read()}const subject=new Renamed();expect(subject.value).toBe(1);});
