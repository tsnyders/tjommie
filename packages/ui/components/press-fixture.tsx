import { isValidElement, type ComponentProps, type MouseEvent, type ReactNode } from "react";
import type * as Controls from "./controls";
import type * as Pills from "./pill";

// Test-only. Server rendering runs no events, so a test swaps these in for the real Pill and Switch (vi.mock with the module's original): each
// still renders as itself and keeps its handler under its label (aria-label, else its text) for the test to press. A disabled one can't be
// pressed, as on screen. Handlers are kept per module instance, so a test file using this doesn't reset modules.
const handlers = new Map<string, (checked: boolean) => unknown>();

export function press(label: string, checked = true): unknown {
  const handler = handlers.get(label);
  if (!handler) throw new Error(`Nothing labelled "${label}" to press. There is: ${[...handlers.keys()].join(" | ")}`);
  return handler(checked);
}

export const clearPresses = (): void => handlers.clear();

const textOf = (node: ReactNode): string => {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  return isValidElement<{ children?: ReactNode }>(node) ? textOf(node.props.children) : "";
};

const keep = (label: string, disabled: boolean | undefined, handler: (checked: boolean) => unknown): void => {
  handlers.set(label, disabled ? () => { throw new Error(`"${label}" is disabled`); } : handler);
};

export function recordingPill(real: typeof Pills): typeof Pills {
  return {
    ...real,
    Pill: (props: ComponentProps<typeof real.Pill>) => {
      const { onClick } = props;
      if (onClick) keep(props["aria-label"] ?? textOf(props.children).trim(), props.disabled, () => onClick({} as MouseEvent<HTMLButtonElement>));
      return real.Pill(props);
    },
  };
}

export function recordingSwitch(real: typeof Controls): typeof Controls {
  return {
    ...real,
    Switch: (props: ComponentProps<typeof real.Switch>) => {
      keep(props.label, props.disabled, props.onChange);
      return real.Switch(props);
    },
  };
}
