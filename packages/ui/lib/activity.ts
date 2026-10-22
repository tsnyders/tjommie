import { ActivityData, type ActivityItem, type Message } from "@tjommie/protocol";

export type RecentStep = { key: string; summary: string; status: ActivityItem["status"]; at: string };

// The bot panel's Recent activity: the newest steps from the chat's activity rows, newest first. Each step
// shows the time its turn's row started.
export function recentActivity(messages: Message[], limit = 8): RecentStep[] {
  const steps: RecentStep[] = [];
  for (let index = messages.length - 1; index >= 0 && steps.length < limit; index--) {
    const message = messages[index];
    if (message.kind !== "activity") continue;
    const parsed = ActivityData.safeParse(message.data);
    if (!parsed.success) continue;
    for (const item of [...parsed.data.items].reverse()) {
      if (steps.length === limit) break;
      steps.push({ key: `${message.id}:${item.id}`, summary: item.summary, status: item.status, at: message.createdAt });
    }
  }
  return steps;
}
