// The other direction of the ECMAScript number-to-string rule for object keys: numeric 1e21
// and the string '1e+21' spell ONE runtime key, so tables and fixture values written either way
// bind the same thing and identical-bodied tests are genuinely redundant. Before the rule
// reached keys, the numeric spelling folded to a long decimal and the pair stayed split. The
// distinct fixture holds the other direction: '1000000000000000000000' is a different key
// however similar it looks.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L15 "twin exponent each"
// @vitest-dedup:participant L16 "twin string each"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L19 "twin exponent extend"
// @vitest-dedup:participant L20 "twin string extend"
import {test,expect} from 'vitest';
test.each([[{1e21:1}]])('twin exponent each',v=>{expect(v['1e+21']).toBe(1)});
test.each([[{'1e+21':1}]])('twin string each',v=>{expect(v['1e+21']).toBe(1)});
const twinExponent=test.extend({row:{1e21:2}});
const twinString=test.extend({row:{'1e+21':2}});
twinExponent('twin exponent extend',({row})=>{expect(Object.is(row['1e+21'],2)).toBe(true)});
twinString('twin string extend',({row})=>{expect(Object.is(row['1e+21'],2)).toBe(true)});
