// A table key that spells the rendering's framing must not collapse onto the structure it spells.
// Expected verdict: parameterization-candidate at warning, NOT duplicate-test. The substituted
// row values differ at `alpha` (the canonical object rendering length-prefixes keys), while
// `structural` keeps only the `<object>` kind -- so the pair stays a legitimate consolidation
// candidate, which is why these expectations are not `rule=none` and carry no `isolates` claim.
// Bodies are deliberately structurally distinct across pairs so each pair forms its own group.
// @vitest-dedup:extracted 6
// @vitest-dedup:not-runnable Each pair contains an intentional runtime failure: the hostile-key row's Object.keys differ from the expected keys.
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L19 "two keys first"
// @vitest-dedup:participant L20 "merged key first"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L21 "two keys through a local"
// @vitest-dedup:participant L22 "merged key through a local"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L23 "nested two keys"
// @vitest-dedup:participant L24 "nested merged key"
import {test,expect} from 'vitest';
test.each([[{a:[],b:1}]])('two keys first',v=>{expect(Object.keys(v)).toEqual(['a','b'])});
test.each([[{'a:[],b':1}]])('merged key first',v=>{expect(Object.keys(v)).toEqual(['a','b'])});
test.each([[{a:[],b:1}]])('two keys through a local',v=>{const keys=Object.keys(v);expect(keys).toEqual(['a','b'])});
test.each([[{'a:[],b':1}]])('merged key through a local',v=>{const keys=Object.keys(v);expect(keys).toEqual(['a','b'])});
test.each([[{outer:{a:[],b:1}}]])('nested two keys',v=>{expect(Object.keys(v.outer)).toEqual(['a','b'])});
test.each([[{outer:{'a:[],b':1}}]])('nested merged key',v=>{expect(Object.keys(v.outer)).toEqual(['a','b'])});
