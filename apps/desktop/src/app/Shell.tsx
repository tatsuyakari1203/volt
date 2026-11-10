import { useEffect, useRef, useState } from 'react';
import { HomeScreen } from '../features/home/HomeScreen';
import { TweaksScreen } from '../features/tweaks/TweaksScreen';
import { NavRail } from './NavRail';
import { PlaceholderScreen } from './PlaceholderScreen';
import { SettingsScreen } from './SettingsScreen';
import { footerSections, initialSection, mainSections, type SectionId } from './sections';

const allSections = [...mainSections, ...footerSections];

function Screen({ id }: { id: SectionId }) {
  switch (id) {
    case 'home':
      return <HomeScreen />;
    case 'tweaks':
      return <TweaksScreen />;
    case 'settings':
      return <SettingsScreen />;
    default: {
      const section = allSections.find((candidate) => candidate.id === id);
      return section ? <PlaceholderScreen section={section} /> : null;
    }
  }
}

/** The app frame: a non-scrolling navigation rail beside a scrolling content area. */
export function Shell() {
  const [current, setCurrent] = useState<SectionId>(() => initialSection(window.location.search));
  const mainRef = useRef<HTMLElement>(null);
  const shown = useRef(current);

  // After navigating, move focus to the content so keyboard and screen-reader users land on it.
  // Not on first render: the window should open with nothing focused.
  useEffect(() => {
    if (shown.current === current) return;
    shown.current = current;
    mainRef.current?.focus({ preventScroll: true });
  }, [current]);

  return (
    <div className="flex h-full min-h-0">
      <NavRail current={current} onNavigate={setCurrent} />
      <main
        ref={mainRef}
        tabIndex={-1}
        aria-label={allSections.find((section) => section.id === current)?.label}
        className="min-w-0 flex-1 overflow-y-auto outline-none"
      >
        <div className="mx-auto w-full max-w-3xl px-gutter pt-4 pb-8 wide:px-6 wide:pt-6">
          <Screen id={current} />
        </div>
      </main>
    </div>
  );
}
