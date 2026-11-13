/** Returns data with duplicate keys hidden behind an owned alias. */
export function make() {
  const nested = { run: () => 1, run: () => 2 }
  return { nested }
}

/** Returns an unambiguous object behind the same alias shape. */
export function valid() {
  const nested = { run: () => 1 }
  return { nested }
}

/** Returns method syntax hidden behind an owned array alias. */
export function arrayMethod() {
  const nested = [{ run() { return 1 } }]
  return { nested }
}
