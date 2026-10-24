// Where the phone layout puts navigation (below 640 px): a floating pill on the overview screens, a Back link on the rest.

const trimmed = (pathname: string): string => pathname.replace(/\/+$/, "") || "/";

// The pill shows on the overview screens (Agents, Team room, Settings and its sections), and not where a composer or an action footer sits
// at the bottom: chat, New bot and Bot settings.
export function showsTabBar(pathname: string): boolean {
  const path = trimmed(pathname);
  return path === "/" || path === "/agents" || path === "/room" || path === "/settings" || path.startsWith("/settings/");
}

export type TabId = "agents" | "room" | "settings";
// `label` is the name for a screen reader and `short` what shows under the icon.
export const TABS: { id: TabId; href: string; label: string; short: string }[] = [
  { id: "agents", href: "/agents/", label: "Agents", short: "Agents" },
  { id: "room", href: "/room/", label: "Team room", short: "Team" },
  { id: "settings", href: "/settings/", label: "Settings", short: "Settings" },
];

// The tab a page belongs to; the first-run page at "/" belongs to none.
export function tabFor(pathname: string): TabId | null {
  const path = trimmed(pathname);
  if (path === "/agents") return "agents";
  if (path === "/room") return "room";
  if (path === "/settings" || path.startsWith("/settings/")) return "settings";
  return null;
}

// The Agents tab's name: it says how many bots wait for the person, since the dot is only a picture.
export const agentsTabName = (waiting: number): string => (waiting > 0 ? `Agents, ${waiting} waiting for you` : "Agents");

// Where "/" goes: a phone with bots to Agents (there is no rail to pick a chat from), a desktop window to the first chat, and a first run
// nowhere.
export function homeTarget(firstBotId: string | undefined, phone: boolean): string | null {
  if (!firstBotId) return null;
  return phone ? "/agents/" : `/chat/?bot=${encodeURIComponent(firstBotId)}`;
}
