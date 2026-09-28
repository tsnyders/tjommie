import type { BotColor, BotShape } from "@tjommie/protocol";

// Bot colors are the only decorative color in the app (docs/design/brief.md section 3).
export const BOT_COLORS: Record<BotColor, string> = {
  brown: "#8D6541",
  red: "#EA4144",
  orange: "#EC712E",
  amber: "#F19C3F",
  green: "#5BC57A",
  teal: "#57B8A7",
  blue: "#3B82F6",
  purple: "#8C5BF3",
  pink: "#EC4698",
  gray: "#777777",
};

// One path per shape and two eye strokes, in a 40 x 40 box (docs/design/brief.md section 5).
const SHAPES: Record<BotShape, { body: string; eyes: string }> = {
  circle: {
    body: "M20 2 C29.94 2 38 10.06 38 20 C38 29.94 29.94 38 20 38 C10.06 38 2 29.94 2 20 C2 10.06 10.06 2 20 2 Z",
    eyes: "M17 14 L16 20 M24.5 14 L23.5 20",
  },
  blob: {
    body: "M4 22 C4 12 11.5 6 21 6 C30.5 6 37 12.5 37 21 C37 29.5 30.5 35 21 35 C11 35 4 30.5 4 22 Z",
    eyes: "M18 15 L17 21 M25.5 15 L24.5 21",
  },
  "rounded-square": {
    body: "M11 4 H29 C32.9 4 36 7.1 36 11 V29 C36 32.9 32.9 36 29 36 H11 C7.1 36 4 32.9 4 29 V11 C4 7.1 7.1 4 11 4 Z",
    eyes: "M17 14 L16 20 M24.5 14 L23.5 20",
  },
  pill: {
    body: "M12 10 H28 C33.5 10 38 14.5 38 20 C38 25.5 33.5 30 28 30 H12 C6.5 30 2 25.5 2 20 C2 14.5 6.5 10 12 10 Z",
    eyes: "M18 15.5 L17.3 21.5 M24.5 15.5 L23.8 21.5",
  },
  triangle: {
    body: "M20 4 C21.6 4 22.8 4.8 23.7 6.3 L37 29.8 C38.6 32.6 36.6 35 33.5 35 H6.5 C3.4 35 1.4 32.6 3 29.8 L16.3 6.3 C17.2 4.8 18.4 4 20 4 Z",
    eyes: "M17.8 20 L17.1 26 M23.8 20 L23.1 26",
  },
  hexagon: {
    body: "M17 3.2 C18.9 2.1 21.1 2.1 23 3.2 L33.5 9.3 C35.4 10.4 36.5 12.4 36.5 14.6 V25.4 C36.5 27.6 35.4 29.6 33.5 30.7 L23 36.8 C21.1 37.9 18.9 37.9 17 36.8 L6.5 30.7 C4.6 29.6 3.5 27.6 3.5 25.4 V14.6 C3.5 12.4 4.6 10.4 6.5 9.3 Z",
    eyes: "M17 15 L16 21 M24.5 15 L23.5 21",
  },
  cloud: {
    body: "M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z",
    eyes: "M17.5 19 L16.7 25 M24.5 19 L23.7 25",
  },
  droplet: {
    body: "M20 3 C20 3 33 16 33 25 C33 32.2 27.2 37 20 37 C12.8 37 7 32.2 7 25 C7 16 20 3 20 3 Z",
    eyes: "M17.5 21 L16.8 27 M24 21 L23.3 27",
  },
};

function Shape({ color, shape }: { color: BotColor; shape: BotShape }) {
  const { body, eyes } = SHAPES[shape];
  return (
    <>
      <path d={body} fill={BOT_COLORS[color]} />
      <path d={eyes} stroke="#FFFFFF" strokeWidth={3} strokeLinecap="round" fill="none" />
    </>
  );
}

export function BotAvatar({ color, shape, size = 40, className }: { color: BotColor; shape: BotShape; size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 40 40" width={size} height={size} className={className} aria-hidden="true">
      <Shape color={color} shape={shape} />
    </svg>
  );
}

// A bot that no longer exists, where its avatar would be.
export function MissingAvatar({ size = 18 }: { size?: number }) {
  return <span aria-hidden="true" className="inline-block shrink-0 rounded-full bg-selected" style={{ width: size, height: size }} />;
}

// The Team room mark: the rail's first item and WF2's Team room card. Three 14 px avatars on a #262626 tile, placed
// as the wireframes draw them. The app icon (packages/desktop/assets/icon.svg) is an older copy on a darker tile.
export function TeamMark({ size = 40, dimmed = false }: { size?: number; dimmed?: boolean }) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} aria-hidden="true" style={{ opacity: dimmed ? 0.4 : 1 }}>
      <rect width="64" height="64" rx="14" fill="var(--pill)" />
      <g transform="translate(9.2 10.8) scale(0.56)">
        <Shape color="purple" shape="circle" />
      </g>
      <g transform="translate(31.5 11.3) scale(0.56)">
        <Shape color="orange" shape="cloud" />
      </g>
      <g transform="translate(20.4 30) scale(0.56)">
        <Shape color="green" shape="rounded-square" />
      </g>
    </svg>
  );
}
