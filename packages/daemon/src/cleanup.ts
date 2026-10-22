// Ordered teardown must reach later resources even when an earlier close fails.
export async function runCleanup(steps: ReadonlyArray<() => void | Promise<void>>, message: string): Promise<void> {
  const errors: unknown[] = [];
  for (const step of steps) {
    try { await step(); } catch (error) { errors.push(error); }
  }
  if (errors.length) throw new AggregateError(errors, message);
}

// Independent children can close together, but reporting an error must wait for every close to settle.
export async function settleCleanup(pending: ReadonlyArray<Promise<void>>, message: string): Promise<void> {
  const results = await Promise.allSettled(pending);
  const errors = results.filter((result): result is PromiseRejectedResult => result.status === "rejected").map((result) => result.reason as unknown);
  if (errors.length) throw new AggregateError(errors, message);
}
