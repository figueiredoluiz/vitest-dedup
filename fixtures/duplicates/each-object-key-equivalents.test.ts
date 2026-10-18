// Tables that spell the same keys differently retain duplicate detection.
// Expected verdict: duplicate-test at error. The canonical rendering is deterministic: quoted and
// identifier spellings of one key, and whitespace variation, fold to one substituted body.
// Bodies are deliberately structurally distinct across pairs so each pair forms its own group.
// @vitest-dedup:extracted 6
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L16 "quoted keys first"
// @vitest-dedup:participant L17 "identifier keys first"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L18 "hostile key first"
// @vitest-dedup:participant L19 "hostile key second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L20 "spaced value first"
// @vitest-dedup:participant L21 "compact value first"
import {test,expect} from 'vitest';
test.each([[{'a':[],'b':1}]])('quoted keys first',v=>{expect(Object.keys(v).join()).toBe('a,b')});
test.each([[{a:[],b:1}]])('identifier keys first',v=>{expect(Object.keys(v).join()).toBe('a,b')});
test.each([[{'a,b':1}]])('hostile key first',v=>{const keys=Object.keys(v);expect(keys).toEqual(['a,b'])});
test.each([[{'a,b':1}]])('hostile key second',v=>{const keys=Object.keys(v);expect(keys).toEqual(['a,b'])});
test.each([[{ a: [ 1 ], b: 2 }]])('spaced value first',v=>{expect(v.b).toBe(2)});
test.each([[{a:[1],b:2}]])('compact value first',v=>{expect(v.b).toBe(2)});
