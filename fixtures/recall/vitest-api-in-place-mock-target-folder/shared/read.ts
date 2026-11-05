/** The value `a/dep` exports when it is loaded, after any mock registered for it. */
export const read = async () => (await import('../a/dep')).value
