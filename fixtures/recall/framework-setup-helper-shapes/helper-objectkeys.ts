// Loops over `Object.keys(...)`.

/** Setup helper `setupObjectKeys` of this case, called from its test file(s). */
export function setupObjectKeys() {
  Object.keys({ a: true, b: false }).forEach((name) => {
    console.info(name)
  })
}
