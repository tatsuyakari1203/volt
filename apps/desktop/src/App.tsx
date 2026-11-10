import { AppInfo } from './features/app-info/AppInfo';

/** Application root. Features are mounted here and nowhere else. */
export function App() {
  return (
    <main className="flex min-h-screen items-center justify-center bg-neutral-950 text-neutral-100">
      <AppInfo />
    </main>
  );
}
