import { describe, expect, test } from 'vitest'

describe.each([['default'], ['with-variant'], ['important'], ['prefix']])('%s', (strategy) => {
  let testName = '%s → %s (%#)'
  if (strategy === 'with-variant') {
    testName = testName.replaceAll('%s', 'focus:%s')
  } else if (strategy === 'important') {
    testName = testName.replaceAll('%s', '%s!')
  } else if (strategy === 'prefix') {
    testName = testName.replaceAll('%s', 'tw:%s')
  }

  /** The candidate as this strategy spells it. */
  function prepare(candidate: string) {
    return strategy === 'default' ? candidate : `${strategy}:${candidate}`
  }

  test.each([
    ['flex', 'flex'],
    ['grid', 'block'],
    ['flex', 'flex'],
  ])(testName, (input, expected) => {
    expect(prepare(input)).toContain(expected)
  })
})

describe.each([['plain'], ['important']])('repeat %s', (strategy) => {
  let testName = '%s → %s'
  if (strategy === 'important') {
    testName = testName.replaceAll('%s', '%s!')
  }

  test.each([
    ['flex', 'flex'],
    ['grid', 'block'],
    ['flex', 'flex'],
  ])(testName, (input, expected) => {
    expect(input).toContain(expected)
  })
})
