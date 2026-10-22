// Numeric object keys follow the ECMAScript number-to-string rule: {1e21: 1} binds the
// runtime key "1e+21", which is a DIFFERENT key than the string
// '1000000000000000000000' however similar the two spellings look. Before keys took the
// ECMAScript spelling, both folded through one decimal rendering and identical-bodied tests
// reported duplicate-test at error severity on keys the runtime keeps apart. The `.each` pair
// below differs only in that key spelling: alpha keeps them apart (parameterization-candidate,
// not duplicate-test) and the `test.extend` pair carries the folded value in its context, so no
// finding may link them at all. The equivalents fixture holds the other direction: numeric 1e21
// and the string '1e+21' are ONE key at runtime and must fold together.
// @vitest-dedup:extracted 4
// @vitest-dedup:not-runnable Each pair contains an intentional runtime failure: the decimal-string row and fixture bind Object.keys the assertion does not expect.
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L20 "exponent each"
// @vitest-dedup:participant L21 "decimal each"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates context
// @vitest-dedup:participant L24 "exponent extend"
// @vitest-dedup:participant L25 "decimal extend"
import {test,expect} from 'vitest';
test.each([[{1e21:1}]])('exponent each',v=>{expect(Object.keys(v).join()).toBe('1e+21')});
test.each([[{'1000000000000000000000':1}]])('decimal each',v=>{expect(Object.keys(v).join()).toBe('1e+21')});
const exponentExtend=test.extend({row:{1e21:2}});
const decimalExtend=test.extend({row:{'1000000000000000000000':2}});
exponentExtend('exponent extend',({row})=>{const got=row['1e+21'];expect(Object.is(got,2)).toBe(true)});
decimalExtend('decimal extend',({row})=>{const got=row['1e+21'];expect(Object.is(got,2)).toBe(true)});
