import { parsePairingCode } from "@tjommie/protocol";

// Why a pasted or scanned pairing code can't be used, or null. It is read only to say so: what the code holds is never shown, stored or logged here.
export function pairingProblem(text: string, now: number = Date.now()): string | null {
  const trimmed = text.trim();
  if (trimmed === "") return null; // nothing typed yet is not a problem, only not ready
  try {
    parsePairingCode(trimmed, now);
    return null;
  } catch {
    return "This pairing code is invalid or expired.";
  }
}

// Whether a code has been typed that looks right, so Connect can be pressed.
export const pairingReady = (text: string, now: number = Date.now()): boolean => text.trim() !== "" && pairingProblem(text, now) === null;

// "9:41" while there is time, "Expired" once there is none. The code works once and for ten minutes.
export function countdown(expires: string, now: number): { text: string; expired: boolean } {
  const left = Math.max(0, Date.parse(expires) - now);
  if (left === 0) return { text: "Expired", expired: true };
  const seconds = Math.ceil(left / 1000);
  return { text: `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`, expired: false };
}
