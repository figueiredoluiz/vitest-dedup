// RECALL negative with a positive control -- parentheses that end an optional chain.
//
// `config?.account.plan` is one chain: a nullish `config` short-circuits the whole expression to
// `undefined`. `(config?.account).plan` evaluates the chain first and then reads `.plan` of its
// result, which throws when `config` is nullish. Both read 'pro' while `config` is defined, so the
// two tests are one edit apart and pass together today; they diverge the day `config` can be
// absent, which is exactly what the nullish-guard matrix treats as a different test.
//
// The third test repeats the first, so a detector that stopped reporting duplicates here would
// fail on the control instead of passing on the negative.
//
// Named weakening: remove the parentheses from the first test and all three become one body.
import { expect, test } from 'vitest'

interface Config {
  account: { plan: string }
}

const config = { account: { plan: 'pro' } } as Config | undefined

test('reads the plan with the chain cut off by parentheses', () => {
  expect((config?.account).plan).toBe('pro')
})

test('reads the plan through one unbroken chain', () => {
  expect(config?.account.plan).toBe('pro')
})

test('reads the plan with the chain cut off here too', () => {
  expect((config?.account).plan).toBe('pro')
})
