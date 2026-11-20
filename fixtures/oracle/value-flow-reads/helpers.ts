import { test } from 'vitest'
export * from './other'
/** Spelled beside a star: its call returns `test`. */
export function make() { return test }
/** The same spelling, returning no registration function. */
export function quiet() { return (..._args: unknown[]) => {} }
