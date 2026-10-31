// `export * as` of the API is its namespace.
import { api } from './namespace'

api.beforeEach(() => {})
api.test('reads as a test', () => {
  api.expect(1).toBe(1)
})
