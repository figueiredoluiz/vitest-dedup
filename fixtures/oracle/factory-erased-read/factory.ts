import {test,expect} from 'vitest'
declare enum MissingEnum { Value }
interface MissingInterface { value: number }
type MissingType = number
/** Read an erased ambient enum before constructing a leaf. */
export function makeEnum(){const unused=MissingEnum;return {run:()=>test('enum',()=>expect(1).toBe(1))}}
/** Read an erased interface before constructing a leaf. */
export function makeInterface(){const unused=MissingInterface;return {run:()=>test('interface',()=>expect(1).toBe(1))}}
/** Read an erased type alias before constructing a leaf. */
export function makeType(){const unused=MissingType;return {run:()=>test('type',()=>expect(1).toBe(1))}}
/** Retain an initialized parameter read and its repeated registration. */
export function makeParameter(value=1){const unused=value;return {run:()=>test('parameter',()=>expect(value).toBe(1))}}
