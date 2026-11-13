import { expect, test } from 'vitest'
import { arrayMethod, make, valid } from './factory'

test('duplicate keys execute the last value', () => { expect(make().nested.run()).toBe(2) })
test('method data', () => { expect(arrayMethod().nested[0].run()).toBe(1) })
test('valid data a', () => { expect(valid().nested.run()).toBe(1) })
test('valid data b', () => { expect(valid().nested.run()).toBe(1) })
