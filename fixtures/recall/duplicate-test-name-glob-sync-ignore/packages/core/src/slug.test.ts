// Ambient globals only. Two tests share a title and check different behavior: a name collision.
import { slug } from './slug'

describe('slug', () => {
  it('normalizes the input', () => {
    expect(slug('Admin User')).toBe('admin-user')
  })

  it('normalizes the input', () => {
    const parts = slug('  Two  Words ').split('-')
    expect(parts).toHaveLength(2)
    expect(parts[0]).not.toContain(' ')
  })
})
