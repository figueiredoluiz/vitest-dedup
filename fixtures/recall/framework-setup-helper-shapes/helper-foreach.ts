// Loops with a callback.
/** Setup helper `setupForEach` of this case, called from its test file(s). */
export function setupForEach() {
  ;['a', 'b'].forEach((name) => {
    console.info(name)
  })
}
