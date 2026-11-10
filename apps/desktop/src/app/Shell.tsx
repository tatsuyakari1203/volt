import { useEffect, useRef, useState } from 'react';
import { CleanScreen } from '../features/clean/CleanScreen';
import { HomeScreen } from '../features/home/HomeScreen';
import { MemoryScreen } from '../features/memory/MemoryScreen';
import { ProcessesScreen } from '../features/processes/ProcessesScreen';
import { StartupScreen } from '../features/startup/StartupScreen';
import { TweaksScreen } from '../features/tweaks/TweaksScreen';
import { UndoScreen } from '../features/undo/UndoScreen';
import { type BoostResult, onBoostFinished } from '../shared/ipc';
import { NavRail } from './NavRail';
import { PlaceholderScreen } from './PlaceholderScreen';
import { SettingsScreen } from './SettingsScreen';
import { footerSections, initialSection, mainSections, type SectionId } from './sections';

const allSections = [...mainSections, ...footerSections];

function Screen({
  id,
  onNavigate,
  trayReport,
}: {
  id: SectionId;
  onNavigate: (id: SectionId) => void;
  trayReport: BoostResult | null;
}) {
  switch (id) {
    case 'home':
      return <HomeScreen onNavigate={onNavigate} trayReport={trayReport} />;
    case 'clean':
      return <CleanScreen />;
    case 'memory':
      return <MemoryScreen />;
    case 'tweaks':
      return <TweaksScreen />;
    case 'startup':
      return <StartupScreen />;
    case 'processes':
      return <ProcessesScreen />;
    case 'settings':
      return <SettingsScreen />;
    case 'undo':
      return <UndoScreen />;
    default: {
      const section = allSections.find((candidate) => candidate.id === id);
      return section ? <PlaceholderScreen section={section} /> : null;
    }
  }
}

/** The app frame: a non-scrolling navigation rail beside a scrolling content area. */
export function Shell() {
  const [current, setCurrent] = useState<SectionId>(() => initialSection(window.location.search));
  const [trayReport, setTrayReport] = useState<BoostResult | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const shown = useRef(current);

  // After navigating, move focus to the content so keyboard and screen-reader users land on it.
  // Not on first render: the window should open with nothing focused.
  useEffect(() => {
    if (shown.current === current) return;
    shown.current = current;
    mainRef.current?.focus({ preventScroll: true });
  }, [current]);

  // The tray's Boost item runs Boost in the shell, with or without a window. A window that is open
  // gets the report and shows it on Home. Outside the Tauri window (a plain browser) there is no
  // bridge and nothing to listen to.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    onBoostFinished((report) => {
      setTrayReport(report);
      setCurrent('home');
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

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
          <Screen id={current} onNavigate={setCurrent} trayReport={trayReport} />
        </div>
      </main>
    </div>
  );
}
