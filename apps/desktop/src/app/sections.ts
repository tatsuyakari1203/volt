import {
  Cpu,
  HeartPulse,
  House,
  type LucideIcon,
  MemoryStick,
  Power,
  Settings,
  SlidersHorizontal,
  Sparkles,
  Undo2,
} from 'lucide-react';

/** Identifier of a top-level section. Navigation is plain local state, not a router. */
export type SectionId =
  | 'home'
  | 'clean'
  | 'memory'
  | 'tweaks'
  | 'startup'
  | 'processes'
  | 'health'
  | 'settings'
  | 'undo';

export interface Section {
  id: SectionId;
  label: string;
  icon: LucideIcon;
}

/** Sections in the upper part of the navigation rail. */
export const mainSections: readonly Section[] = [
  { id: 'home', label: 'Home', icon: House },
  { id: 'clean', label: 'Clean', icon: Sparkles },
  { id: 'memory', label: 'Memory', icon: MemoryStick },
  { id: 'tweaks', label: 'Tweaks', icon: SlidersHorizontal },
  { id: 'startup', label: 'Startup', icon: Power },
  { id: 'processes', label: 'Processes', icon: Cpu },
  { id: 'health', label: 'Health', icon: HeartPulse },
];

/** Sections pinned to the bottom of the rail. */
export const footerSections: readonly Section[] = [
  { id: 'settings', label: 'Settings', icon: Settings },
  { id: 'undo', label: 'Undo', icon: Undo2 },
];

/**
 * Reads a `?section=<id>` query parameter so a screen can be opened directly (used to capture
 * screenshots). Unknown values fall back to Home.
 */
export function initialSection(search: string): SectionId {
  const value = new URLSearchParams(search).get('section');
  return [...mainSections, ...footerSections].find((section) => section.id === value)?.id ?? 'home';
}
