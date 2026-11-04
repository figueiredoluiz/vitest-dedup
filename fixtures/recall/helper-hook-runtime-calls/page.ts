import { beforeAll, afterAll } from 'vitest'

/** Opens a page around the suite and returns accessors the tests call while they run. */
export function setupPage() {
  let page = 0
  beforeAll(() => {
    page = 1
  })
  afterAll(() => {
    page = 0
  })
  /** The page counter the hooks set. */
  function count() {
    return page
  }
  return { page: () => page, count }
}
