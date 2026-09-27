import type { BotForm, RoutineDraft } from "./bot-form";

export type Suggestion = {
  title: string;
  description: string;
  form: Pick<BotForm, "name" | "color" | "shape" | "instructions" | "browser">;
  routines: RoutineDraft[];
};

const WEEKDAYS = [1, 2, 3, 4, 5];

// The New bot screen's templates (spec section 7, WF3). Picking one fills in the form; nothing is created until
// Create bot, which then creates the routines too.
export const SUGGESTIONS: Suggestion[] = [
  {
    title: "Night Shift",
    description: "Works overnight and preps your morning digest",
    form: {
      name: "Night Shift",
      color: "amber",
      shape: "hexagon",
      browser: false,
      instructions:
        "Each weekday morning, write a short digest in digest.md in your home folder: what's new in your inbox folder, what you finished yesterday, and anything I asked you to keep an eye on. Keep it to one screen, and tell me when it's ready.",
    },
    routines: [
      {
        name: "Morning digest",
        prompt: "Write today's morning digest in digest.md, then tell me it's ready in one line.",
        days: WEEKDAYS,
        times: ["06:30"],
        timeLimitMinutes: 60,
        enabled: true,
      },
    ],
  },
  {
    title: "Inbox Triage",
    description: "Sorts your email and drafts replies in your voice",
    form: {
      name: "Inbox Triage",
      color: "pink",
      shape: "cloud",
      browser: true,
      instructions:
        "Use your browser to check my email; I sign in to it there once. Sort new mail into what needs me, what can wait and what's noise. Draft replies in my voice, but never send anything without asking me first.",
    },
    routines: [
      {
        name: "Inbox sweep",
        prompt: "Check my inbox for new mail, sort it, and draft replies for anything that needs one. Don't send anything.",
        days: WEEKDAYS,
        times: ["08:00", "12:00", "16:00"],
        timeLimitMinutes: 60,
        enabled: true,
      },
    ],
  },
  {
    title: "Job hunter",
    description: "Applies to roles that match your CV every weekday",
    form: {
      name: "Job hunter",
      color: "orange",
      shape: "cloud",
      browser: true,
      instructions:
        "Find jobs that match my CV (I'll put it in your inbox folder) on the job sites I sign in to in your browser. Apply only to roles that fit, skip senior roles unless I say otherwise, and keep a list of what you applied to in applications.md.",
    },
    routines: [
      {
        name: "Job search",
        prompt: "Search for new roles that match my CV and apply to the ones that fit. Add each application to applications.md.",
        days: WEEKDAYS,
        times: ["09:00"],
        timeLimitMinutes: 180,
        enabled: true,
      },
    ],
  },
];
