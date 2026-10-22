// A template-literal title interpolating a composite coerces the way JavaScript's ToString
// coerces it: `obj ${{a: 1}}` is the title "obj [object Object]" and `arr ${[1, 2]}` is
// "arr 1,2". Before titles took the coercion path, the substitution folded to the internal
// canonical object rendering, so the true pairs below (template against the literal spelling of
// the same runtime title) never collided under duplicate-test-name -- and a literal spelling the
// canonical text could have collided falsely. Bodies differ within each pair (an inert local in
// one member) so duplicate-test has nothing to say and the title rule carries both pairs alone.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=duplicate-test-name verdict=name-collision severity=warning
// @vitest-dedup:participant L16 "obj [object Object]"
// @vitest-dedup:participant L17 "obj [object Object]"
// @vitest-dedup:expect rule=duplicate-test-name verdict=name-collision severity=warning
// @vitest-dedup:participant L18 "arr 1,2"
// @vitest-dedup:participant L19 "arr 1,2"
import {test,expect} from 'vitest';
test(`obj ${{a: 1}}`,()=>{expect(1).toBe(1)});
test('obj [object Object]',()=>{const inert=1;expect(1).toBe(1)});
test(`arr ${[1, 2]}`,()=>{expect('arr').toBe('arr')});
test('arr 1,2',()=>{const inert=2;expect('arr').toBe('arr')});
