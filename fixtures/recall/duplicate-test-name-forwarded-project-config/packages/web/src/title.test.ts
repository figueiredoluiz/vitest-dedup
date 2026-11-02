// Ambient globals only. A second name collision, in the project that imports the shared config.
import { title } from './title'

describe('title', () => {
  it('formats the page title', () => {
    expect(title('Home')).toBe('Home | Site')
  })

  it('formats the page title', () => {
    const empty = title('')
    expect(empty.startsWith('Site')).toBe(true)
  })
})
