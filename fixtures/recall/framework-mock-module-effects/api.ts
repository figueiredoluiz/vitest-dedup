// The module the mock modules replace.
/** Setup helper `fetchUser` of this case, called from its test file(s). */
export function fetchUser(): { id: number; name: string } {
  return { id: 0, name: 'real' }
}
