// A declared class emits nothing, so the star's alias is the value; the name is still refused.
import { alias } from './namespace-export-declared'

alias.test('through a star beside a declaration', () => {
  alias.expect(1).toBe(1)
})
