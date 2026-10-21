// Assigns a member of an object that outlives the call.

/** Setup helper `setupMemberAssign` of this case, called from its test file(s). */
export function setupMemberAssign() {
  ;(globalThis as { ready?: boolean }).ready = true
}
