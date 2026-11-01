import { expect, test } from 'vitest'

const consume = (value: unknown) => value
const key = () => 'value'
const handlers = { a: () => expect(1).toBe(1), b: () => expect(2).toBe(1) }
const tag = (parts: TemplateStringsArray) => expect(parts[0]).toBe('a')

test('branch eager', () => { const x = expect(1).toBe(2); if (false) consume(x) })
test('branch moved', () => { if (false) consume(expect(1).toBe(2)) })
test('closure eager', () => { const x = expect(1).toBe(2); function later() { consume(x) } })
test('closure moved', () => { function later() { consume(expect(1).toBe(2)) } })
test('tdz eager', () => { const x = y; const y = 1; expect(x === y).toBe(true) })
test('tdz moved', () => { const y = 1; expect(y === y).toBe(true) })
test('class a', () => { class C { [key()]() { return 1 } }; expect(new C().value()).toBe(1) })
test('class b', () => { class C { [key()]() { return 2 } }; expect(new C().value()).toBe(1) })
test('class alias a', () => { class C { [key()]() { return 1 } }; const D = C; expect(new D().value()).toBe(1) })
test('class alias b', () => { class C { [key()]() { return 2 } }; const D = C; expect(new D().value()).toBe(1) })
test('call a', () => { handlers['a']() })
test('call b', () => { handlers['b']() })
test('tag a', () => { tag`a`; expect(1).toBe(1) })
test('tag b', () => { tag`b`; expect(1).toBe(1) })
test('spread a', () => { const value = [...[expect(1).toBe(1)]]; expect(1).toBe(1) })
test('spread b', () => { const value = [...[expect(2).toBe(1)]]; expect(1).toBe(1) })
test('chai a', () => { expect(true).to.be.true })
test('chai b', () => { expect(false).to.be.true })
test('positive a', () => { const x = handlers.a(); consume(x) })
test('positive b', () => { consume(handlers.a()) })
test('if pass', () => { if (true) { expect(1).toBe(1) } else { expect(2).toBe(1) } })
test('if fail', () => { if (true) { expect(2).toBe(1) } else { expect(1).toBe(1) } })
test('try pass', () => { try { expect(2).toBe(1) } catch {}; expect(1).toBe(1) })
test('try fail', () => { try {} catch {}; expect(2).toBe(1); expect(1).toBe(1) })
test('literal one', () => { expect(10).toBe(10) })
test('literal two', () => { expect(20).toBe(20) })
test('flat one', () => { if (false) expect(10).toBe(20) })
test('flat two', () => { if (false) expect(10).toBe(20) })

test('delete pass', () => {
  const target: { value?: number } = { value: 1 }
  delete target.value
  expect(target).toBeDefined()
  expect(target.value).toBeUndefined()
})
test('delete fail', () => {
  const target: { value?: number } = { value: 1 }
  expect(target).toBeDefined()
  expect(target.value).toBeUndefined()
})
test('using pass', () => {
  const log: string[] = []
  {
    using resource = { [Symbol.dispose]: () => { log.push('disposed') } }
    expect(resource).toBeDefined()
  }
  expect(log).toEqual(['disposed'])
})
test('using fail', () => {
  const log: string[] = []
  {
    const resource = { [Symbol.dispose]: () => { log.push('disposed') } }
    expect(resource).toBeDefined()
  }
  expect(log).toEqual(['disposed'])
})
