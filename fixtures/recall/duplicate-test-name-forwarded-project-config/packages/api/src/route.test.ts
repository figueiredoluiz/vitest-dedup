// Ambient globals only. Two tests share a title and check different behavior: a name collision.
import { route } from './route'

describe('route', () => {
  it('matches the path', () => {
    expect(route('/users/7')).toEqual({ name: 'user', id: '7' })
  })

  it('matches the path', () => {
    const matched = route('/users/')
    expect(matched).toBeNull()
  })
})
