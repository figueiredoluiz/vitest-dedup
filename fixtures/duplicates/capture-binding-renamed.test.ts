// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';
test('first',()=>{void 'capture-binding-renamed';const a=[];const b=[];class C{constructor(){b.push(1)}}new C();expect(b).toEqual([1]);});
test('second',()=>{void 'capture-binding-renamed';const unused=[];const result=[];class Renamed{constructor(){result.push(1)}}new Renamed();expect(result).toEqual([1]);});
