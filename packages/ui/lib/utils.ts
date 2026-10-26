export { cn } from "cn"

// Waits for the work for at most `ms`, and never fails: true when it settled in time.
export async function atMost(ms: number, work: Promise<unknown>): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const settled = await Promise.race([work.then(() => true, () => true), new Promise<boolean>((resolve) => { timer = setTimeout(() => resolve(false), ms); })]);
  clearTimeout(timer);
  return settled;
}
