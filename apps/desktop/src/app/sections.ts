import {
  AppWindow,
  Cpu,
  House,
  type LucideIcon,
  MemoryStick,
  Power,
  Settings,
  SlidersHorizontal,
  Sparkles,
  Undo2,
} from 'lucide-react';
import type { MessageKey } from '../shared/i18n';

/** Identifier of a top-level section. Navigation is plain local state, not a router. */
export type SectionId =
  | 'home'
  | 'clean'
  | 'memory'
  | 'tweaks'
  | 'startup'
  | 'apps'
  | 'processes'
  | 'settings'
  | 'undo';

export interface Section {
  id: SectionId;
  /** Key of the name shown in the navigation rail and as the title of the page. */
  label: MessageKey;
  icon: LucideIcon;
}

/** Sections in the upper part of the navigation rail. */
export const mainSections: readonly Section[] = [
  { id: 'home', label: 'nav.home', icon: House },
  { id: 'clean', label: 'nav.clean', icon: Sparkles },
  { id: 'memory', label: 'nav.memory', icon: MemoryStick },
  { id: 'tweaks', label: 'nav.tweaks', icon: SlidersHorizontal },
  { id: 'startup', label: 'nav.startup', icon: Power },
  { id: 'apps', label: 'nav.apps', icon: AppWindow },
  { id: 'processes', label: 'nav.processes', icon: Cpu },
];

/** Sections pinned to the bottom of the rail. */
export const footerSections: readonly Section[] = [
  { id: 'settings', label: 'nav.settings', icon: Settings },
  { id: 'undo', label: 'nav.undo', icon: Undo2 },
];

/**
 * Reads a `?section=<id>` query parameter so a screen can be opened directly (used to capture
 * screenshots). Unknown values fall back to Home.
 */
export function initialSection(search: string): SectionId {
  const value = new URLSearchParams(search).get('section');
  return [...mainSections, ...footerSections].find((section) => section.id === value)?.id ?? 'home';
}
