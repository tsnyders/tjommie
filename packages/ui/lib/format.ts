const CLOCK = new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit" });
const WEEKDAY = new Intl.DateTimeFormat("en-US", { weekday: "long" });
const DAY_MONTH = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "long" });
const DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DAY_MS = 86_400_000;

// "9:14 AM". Newer ICU puts a narrow no-break space before AM; the wireframes use a plain space.
export const clock = (value: string | Date): string => CLOCK.format(new Date(value)).replace(/\u202f/g, " ");

const startOfDay = (date: Date): number => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

// "Today", "Yesterday", a weekday within the last week, otherwise "21 September".
export function dayName(value: string | Date, now: Date = new Date()): string {
  const date = new Date(value);
  const days = Math.round((startOfDay(now) - startOfDay(date)) / DAY_MS);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days > 1 && days < 7) return WEEKDAY.format(date);
  return DAY_MONTH.format(date);
}

// "Today 9:14 AM": the time labels between messages.
export const stamp = (value: string | Date, now?: Date): string => `${dayName(value, now)} ${clock(value)}`;

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

// "Weekdays at 8:48 AM", "Every day at 7:00 AM, 5:00 PM", "Mon, Wed at 9:00 AM".
export function schedule(days: number[], times: string[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const key = sorted.join("");
  const when = key === "0123456" ? "Every day" : key === "12345" ? "Weekdays" : key === "06" ? "Weekends" : sorted.map((day) => DAY_SHORT[day]).join(", ");
  const at = times.map((time) => clock(new Date(2000, 0, 1, Number(time.slice(0, 2)), Number(time.slice(3))))).join(", ");
  return `${when} at ${at}`;
}

// "Pnet apply 9am (3h)": a routine's name with its time limit.
export function routineTitle(name: string, limitMinutes: number | null): string {
  if (!limitMinutes) return name;
  return `${name} (${limitMinutes % 60 === 0 ? `${limitMinutes / 60}h` : `${limitMinutes}m`})`;
}
