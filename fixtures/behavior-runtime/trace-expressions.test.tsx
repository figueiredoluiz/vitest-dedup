/** @jsxRuntime classic */
/** @jsx create */
/** @jsxFrag Fragment */
import { expect, test } from 'vitest'

const Fragment = 'fragment'
const create = (name: string, props: unknown, ...children: unknown[]) => {
  expect(name).not.toBe('bad')
  return { name, props, children }
}

test('jsx pass', () => { <good />; expect(1).toBe(1) })
test('jsx fail', () => { <bad />; expect(1).toBe(1) })
test('fragment pass', () => { <><good /></>; expect(1).toBe(1) })
test('fragment fail', () => { <><bad /></>; expect(1).toBe(1) })
test('yield pass', () => { function* gen() { yield expect(1).toBe(1) }; [...gen()] })
test('yield fail', () => { function* gen() { yield expect(2).toBe(1) }; [...gen()] })
test('object spread pass', () => { const copy = { ...{ get value() { expect(1).toBe(1); return 1 } } }; expect(1).toBe(1) })
test('object spread fail', () => { const copy = { ...{ get value() { expect(2).toBe(1); return 1 } } }; expect(1).toBe(1) })
test('import pass', async () => { const { value } = await import('./trace-value'); expect(value).toBe(true) })
test('import fail', async () => { const { value } = await import('./trace-value'); expect(value).toBe(false) })
