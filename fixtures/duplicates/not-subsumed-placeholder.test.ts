// A placeholder assertion is not evidence for optional subsumption.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=not-compared severity=none
// @vitest-dedup:isolates not-isolable the default engine excludes the canonical placeholder as trivial; the owned Rust control retains a real one-event assertion and requires the placeholder to have no subsumption key
// @vitest-dedup:participant L9 "placeholder"
// @vitest-dedup:participant L10 "observations"
import { test, expect } from 'vitest'

test('placeholder', () => expect(true).toBe(true))
test('observations', () => { expect(true).toBe(true); expect(2).toBe(2) })
