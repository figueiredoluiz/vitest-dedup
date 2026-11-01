globalThis.hookBoundaryWrites = 0
Object.defineProperty(globalThis, 'hookBoundaryState', {
  configurable: true,
  set(_value: number) { globalThis.hookBoundaryWrites += 1 },
})
