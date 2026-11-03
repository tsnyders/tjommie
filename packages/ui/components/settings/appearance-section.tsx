"use client";

import { useState } from "react";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Segmented, Switch } from "@/components/controls";
import { applyDensity, applyTextSize, density, textSize, type Density, type TextSize } from "@/lib/appearance-settings";
import { MOBILE_STYLES, mobileStyle, reduceEffects, type MobileStyle } from "@/lib/mobile-style";
import { applyMobileStyle, applyReduceEffects, applyTheme, themeChoice, type ThemeChoice } from "@/lib/theme";
import { usePhone } from "@/lib/use-phone";
import { SettingsCard, SettingsProblem, SettingsRow, SettingsSection } from "./section";

const THEMES = [{ value: "dark", label: "Dark" }, { value: "light", label: "Light" }, { value: "system", label: "System" }] as const;
const TEXT = [{ value: "system", label: "System" }, { value: "large", label: "Large" }, { value: "larger", label: "Larger" }] as const;
const SPACING = [{ value: "comfortable", label: "Comfortable" }, { value: "compact", label: "Compact" }] as const;

export function AppearanceSection() {
  const [theme, setTheme] = useState<ThemeChoice>(themeChoice);
  const [size, setSize] = useState<TextSize>(textSize);
  const [spacing, setSpacing] = useState<Density>(density);
  const [problem, setProblem] = useState(false);
  const phone = usePhone();
  function save(change: () => void): void {
    try { change(); setProblem(false); } catch { setProblem(true); }
  }
  return <SettingsSection title="Appearance" note="Saved on this device. Switching PCs keeps your appearance choices.">
    <SettingsCard>
      <SettingsRow title="Theme"><Segmented label="Theme" value={theme} options={THEMES} onChange={(next) => save(() => { applyTheme(next); setTheme(next); })} /></SettingsRow>
      <SettingsRow title="Text size" note="System follows your device's default text size. Larger choices also give controls more room."><Segmented label="Text size" value={size} options={TEXT} onChange={(next) => save(() => { applyTextSize(next); setSize(next); })} /></SettingsRow>
      <SettingsRow title="Spacing" note="Compact brings cards and settings rows closer together. Phone touch targets keep their size."><Segmented label="Spacing" value={spacing} options={SPACING} onChange={(next) => save(() => { applyDensity(next); setSpacing(next); })} /></SettingsRow>
    </SettingsCard>
    {phone && <PhoneLook save={save} />}
    {problem && <SettingsProblem>This device could not save the appearance choice. Try again.</SettingsProblem>}
  </SettingsSection>;
}

function PhoneLook({ save }: { save: (change: () => void) => void }) {
  const [style, setStyle] = useState<MobileStyle>(mobileStyle);
  const [reduced, setReduced] = useState(reduceEffects);
  return <SettingsCard>
    <div className="py-4">
      <p className="text-[14px] text-text">Phone style</p>
      <p className="mt-1 text-[12px] leading-[18px] text-secondary">How Tjommie looks on this phone.</p>
      <RadioGroup aria-label="Mobile style" value={style} onValueChange={(value) => save(() => {
        const next = MOBILE_STYLES.find((item) => item.value === value)?.value ?? "hush";
        applyMobileStyle(next); setStyle(next);
      })} className="mt-3 grid grid-cols-3 gap-2">
        {MOBILE_STYLES.map((item) => <Radio.Root key={item.value} value={item.value} aria-label={item.label} className="mobile-motion-choice flex h-[72px] flex-col items-center justify-center gap-2 rounded-[12px] border border-divider text-[13px] text-secondary data-[checked]:border-text data-[checked]:font-semibold data-[checked]:text-text">
          <span aria-hidden data-swatch={item.value} className="style-swatch" />{item.label}
        </Radio.Root>)}
      </RadioGroup>
    </div>
    <SettingsRow title="Reduce effects" note="Solid surfaces and still light in Aurora. System reduced-motion preferences are also respected."><Switch label="Reduce effects" checked={reduced} onChange={(next) => save(() => { applyReduceEffects(next); setReduced(next); })} /></SettingsRow>
  </SettingsCard>;
}
