// This export erases, so the star's alias is the value here; the name is still refused.
import { alias } from './namespace-export-type'

alias.test('through a star beside a type', () => {
  alias.expect(1).toBe(1)
})
