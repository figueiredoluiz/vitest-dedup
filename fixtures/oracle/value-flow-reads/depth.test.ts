import { test } from 'vitest'
import { deep, shallow } from './deep'
deep()('past the deepest chain', () => {})
shallow()('within the deepest chain', () => {})
const n0 = (..._args: unknown[]) => {}
const n1 = (..._args: unknown[]) => {}
const n2 = (..._args: unknown[]) => {}
const n3 = (..._args: unknown[]) => {}
const n4 = (..._args: unknown[]) => {}
const n5 = (..._args: unknown[]) => {}
const n6 = (..._args: unknown[]) => {}
const n7 = (..._args: unknown[]) => {}
const n8 = (..._args: unknown[]) => {}
const n9 = (..._args: unknown[]) => {}
const n10 = (..._args: unknown[]) => {}
const n11 = (..._args: unknown[]) => {}
const n12 = (..._args: unknown[]) => {}
const n13 = (..._args: unknown[]) => {}
const n14 = (..._args: unknown[]) => {}
const n15 = (..._args: unknown[]) => {}
const n16 = (..._args: unknown[]) => {}
const wide = Math.random() > 2 ? n0 : Math.random() > 2 ? n1 : Math.random() > 2 ? n2 : Math.random() > 2 ? n3 : Math.random() > 2 ? n4 : Math.random() > 2 ? n5 : Math.random() > 2 ? n6 : Math.random() > 2 ? n7 : Math.random() > 2 ? n8 : Math.random() > 2 ? n9 : Math.random() > 2 ? n10 : Math.random() > 2 ? n11 : Math.random() > 2 ? n12 : Math.random() > 2 ? n13 : Math.random() > 2 ? n14 : Math.random() > 2 ? n15 : Math.random() > 2 ? n16 : test
wide('past the widest choice', () => {})
