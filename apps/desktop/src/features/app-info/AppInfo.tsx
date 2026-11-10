import { useEffect, useState } from 'react';
import { type AppInfo as AppInfoData, getAppInfo } from '../../shared/ipc';
import { Muted } from '../../shared/ui/Muted';

/** Minimal screen: app name and version, read from Rust. */
export function AppInfo() {
  const [info, setInfo] = useState<AppInfoData | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  if (error) {
    return <Muted>Could not reach the Rust core: {error}</Muted>;
  }
  if (!info) {
    return <Muted>Loading...</Muted>;
  }
  return (
    <div className="text-center">
      <h1 className="text-3xl font-semibold">{info.name}</h1>
      <Muted>Version {info.version}</Muted>
    </div>
  );
}
