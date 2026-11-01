// Computed constant-template property names remain structural dimensions.
// @vitest-dedup:extracted 8
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier,literal
// @vitest-dedup:participant L21 "property a"
// @vitest-dedup:participant L22 "property b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier,literal
// @vitest-dedup:participant L23 "object a"
// @vitest-dedup:participant L24 "object b"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier,literal
// @vitest-dedup:participant L28 "surrogate key"
// @vitest-dedup:participant L29 "replacement key"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L25 "retain zero"
// @vitest-dedup:participant L26 "retain one"

import { test, expect } from 'vitest'
const values = { a: 0, b: 1 }
test('property a', () => { void 'computed-key-member'; expect(values[`a`]).toBe(0) })
test('property b', () => { void 'computed-key-member'; expect(values[`b`]).toBe(1) })
test('object a', () => { void 'computed-key-object'; expect({ [`a`]: 0 }).toBeTruthy() })
test('object b', () => { void 'computed-key-object'; expect({ [`b`]: 1 }).toBeTruthy() })
test('retain zero', () => { void 'computed-key-retention'; expect(values[`a`] + 0).toBe(0) })
test('retain one', () => { void 'computed-key-retention'; expect(values[`a`] + 1).toBe(1) })
const textValues = { ['\uD800']: 0, ['\uFFFDd800']: 1 }
test('surrogate key', () => { void 'computed-key-text'; expect(textValues[`\uD800`]).toBe(0) })
test('replacement key', () => { void 'computed-key-text'; expect(textValues[`\uFFFDd800`]).toBe(1) })
