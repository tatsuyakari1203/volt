import { Zap } from 'lucide-react';
import { cx } from '../shared/ui';
import { footerSections, mainSections, type Section, type SectionId } from './sections';

interface NavRailProps {
  current: SectionId;
  onNavigate: (id: SectionId) => void;
}

/**
 * Primary navigation. An icon rail with tooltips when the window is compact, and a labelled
 * sidebar when it is wide. Labels stay in the DOM at every width, so each button is always named.
 * The rail never scrolls; only the content area does.
 */
export function NavRail({ current, onNavigate }: NavRailProps) {
  return (
    <nav
      aria-label="Primary"
      className="flex w-rail shrink-0 flex-col gap-1 px-2 py-3 wide:w-rail-wide"
    >
      <div className="mb-3 flex h-10 items-center gap-3 px-1.5">
        <span
          aria-hidden
          className="grid size-7 shrink-0 place-items-center rounded-md bg-accent text-accent-fg"
        >
          <Zap size={16} strokeWidth={2.25} />
        </span>
        <span className="sr-only text-subtitle font-semibold wide:not-sr-only">Volt</span>
      </div>
      <ul className="flex flex-col gap-1">
        {mainSections.map((section) => (
          <NavItem
            key={section.id}
            section={section}
            active={section.id === current}
            onNavigate={onNavigate}
          />
        ))}
      </ul>
      <ul className="mt-auto flex flex-col gap-1">
        {footerSections.map((section) => (
          <NavItem
            key={section.id}
            section={section}
            active={section.id === current}
            onNavigate={onNavigate}
          />
        ))}
      </ul>
    </nav>
  );
}

function NavItem({
  section,
  active,
  onNavigate,
}: {
  section: Section;
  active: boolean;
  onNavigate: (id: SectionId) => void;
}) {
  const Icon = section.icon;
  return (
    <li>
      <button
        type="button"
        aria-current={active ? 'page' : undefined}
        onClick={() => onNavigate(section.id)}
        className={cx(
          'group relative flex h-10 w-full items-center gap-3 rounded-md px-2.5 transition-colors duration-(--duration-fast) ease-standard',
          active
            ? 'bg-accent-soft font-semibold text-accent-text'
            : 'text-fg hover:bg-hover active:bg-pressed',
        )}
      >
        {active ? (
          <span
            aria-hidden
            className="absolute top-1/2 left-0 h-4 w-[3px] -translate-y-1/2 rounded-full bg-accent"
          />
        ) : null}
        <Icon aria-hidden size={20} strokeWidth={1.75} className="shrink-0" />
        <span className="sr-only min-w-0 truncate text-body wide:not-sr-only">{section.label}</span>
        <span
          aria-hidden
          className="pointer-events-none absolute top-1/2 left-[calc(100%+8px)] z-50 -translate-y-1/2 whitespace-nowrap rounded-sm bg-toast px-2 py-1 text-caption font-medium text-toast-fg opacity-0 shadow-pop transition-opacity duration-(--duration-fast) group-hover:opacity-100 group-focus-visible:opacity-100 wide:hidden"
        >
          {section.label}
        </span>
      </button>
    </li>
  );
}
