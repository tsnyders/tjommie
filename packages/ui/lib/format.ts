const CLOCK = new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit" });
const WEEKDAY = new Intl.DateTimeFormat("en-US", { weekday: "long" });
const DAY_MONTH = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "long" });
const DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DAY_MS = 86_400_000;

// "9:14 AM". Newer ICU puts a narrow no-break space before AM; the wireframes use a plain space.
export const clock = (value: string | Date): string => CLOCK.format(new Date(value)).replace(/\u202f/g, " ");

const startOfDay = (date: Date): number => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

// "Today", "Yesterday", "Tomorrow", a weekday within a week either way ("Friday", or "Fri" when short), otherwise
// "21 September".
export function dayName(value: string | Date, now: Date = new Date(), short = false): string {
  const date = new Date(value);
  const days = Math.round((startOfDay(now) - startOfDay(date)) / DAY_MS);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days === -1) return "Tomorrow";
  if (Math.abs(days) < 7) return short ? DAY_SHORT[date.getDay()] : WEEKDAY.format(date);
  return DAY_MONTH.format(date);
}

// "Monday 21 September".
export function longDate(value: string | Date): string {
  const date = new Date(value);
  return `${WEEKDAY.format(date)} ${DAY_MONTH.format(date)}`;
}

// "Every day", "Weekdays", "Weekends" or "Mon, Wed".
export function daysLabel(days: number[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const key = sorted.join("");
  if (key === "0123456") return "Every day";
  if (key === "12345") return "Weekdays";
  if (key === "06") return "Weekends";
  return sorted.map((day) => DAY_SHORT[day]).join(", ");
}

// "8:48 AM, 4:45 PM" from "HH:MM" times.
export function timesLabel(times: string[]): string {
  return [...new Set(times)]
    .sort()
    .map((time) => clock(new Date(2000, 0, 1, Number(time.slice(0, 2)), Number(time.slice(3)))))
    .join(", ");
}

// "Weekdays at 8:48 AM", "Every day at 7:00 AM, 5:00 PM", "Mon, Wed at 9:00 AM".
export const schedule = (days: number[], times: string[]): string => `${daysLabel(days)} at ${timesLabel(times)}`;

// "just now", "4 minutes ago", "2 hours ago", "yesterday", or "on Monday".
export function timeAgo(value: string | Date, now: Date = new Date()): string {
  const minutes = Math.floor((now.getTime() - new Date(value).getTime()) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} ${minutes === 1 ? "minute" : "minutes"} ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} ${hours === 1 ? "hour" : "hours"} ago`;
  const day = dayName(value, now);
  return day === "Yesterday" ? "yesterday" : `on ${day}`;
}

// How long something ran: "2h 51m", "3h", "4m" or "under a minute".
export function duration(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (!hours) return `${rest}m`;
  return rest ? `${hours}h ${rest}m` : `${hours}h`;
}

// "Today 9:14 AM": the time labels between messages.
export const stamp = (value: string | Date, now?: Date): string => `${dayName(value, now)} ${clock(value)}`;

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

// "Pnet apply 9am (3h)": a routine's name with its time limit.
export function routineTitle(name: string, limitMinutes: number | null): string {
  if (!limitMinutes) return name;
  return `${name} (${limitMinutes % 60 === 0 ? `${limitMinutes / 60}h` : `${limitMinutes}m`})`;
}
