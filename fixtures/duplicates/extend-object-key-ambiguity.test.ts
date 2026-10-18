// A test.extend fixture value whose key spells the rendering's framing must stay its own context.
// Expected verdict: distinct. The two bodies are identical at every body-level view; only the
// folded fixture value -- part of the ambient context key -- keeps the pairs apart.
// @vitest-dedup:extracted 4
// @vitest-dedup:not-runnable Each pair contains an intentional runtime failure: the hostile-key fixture's Object.keys differ from the expected keys.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates context
// @vitest-dedup:participant L19 "reads two keys"
// @vitest-dedup:participant L20 "reads merged key"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates context
// @vitest-dedup:participant L21 "reads two object values"
// @vitest-dedup:participant L22 "reads merged object values"
import {test,expect} from 'vitest';
const readsTwoKeys=test.extend({row:{a:[],b:1}});
const readsMergedKey=test.extend({row:{'a:[],b':1}});
const readsTwoObjects=test.extend({cfg:{a:{},b:2}});
const readsMergedObjects=test.extend({cfg:{'a:{},b':2}});
readsTwoKeys('reads two keys',({row})=>{expect(Object.keys(row)).toEqual(['a','b'])});
readsMergedKey('reads merged key',({row})=>{expect(Object.keys(row)).toEqual(['a','b'])});
readsTwoObjects('reads two object values',({cfg})=>{expect(Object.keys(cfg)).toEqual(['a','b'])});
readsMergedObjects('reads merged object values',({cfg})=>{expect(Object.keys(cfg)).toEqual(['a','b'])});
