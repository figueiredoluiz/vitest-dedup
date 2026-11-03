import { test as base } from 'vitest'

function makeDb() {
  return 1
}

const test = base.extend({ db: makeDb() })

export default test
