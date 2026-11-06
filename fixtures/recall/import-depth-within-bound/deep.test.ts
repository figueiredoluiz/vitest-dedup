// RECALL positive -- an import chain past eight levels but inside the import-analysis bound.
//
// `m0` reaches `m12` through twelve side-effect imports, none of which registers anything, so the
// identical pair below stays comparable and is reported.
//
// Load-bearing: lower the import-analysis depth below 12 and the file is refused as
// `helper-export-unreadable`, withholding the pair.
import './m0'
import { test, expect } from 'vitest'

test('within bound first', () => { expect(1).toBe(1) })
test('within bound second', () => { expect(1).toBe(1) })
