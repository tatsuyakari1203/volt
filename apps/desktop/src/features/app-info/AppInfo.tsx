import { useEffect, useState } from 'react';
import { t } from '../../shared/i18n';
import { type AppInfo as AppInfoData, getAppInfo, IpcUnavailableError } from '../../shared/ipc';
import { Muted } from '../../shared/ui/Muted';

type Failure = { kind: 'outside-app' } | { kind: 'error'; message: string };

/** Minimal block: app name and version, read from Rust. It sits inside a card of Settings. */
export function AppInfo() {
  const [info, setInfo] = useState<AppInfoData | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch((e: unknown) =>
        setFailure(
          e instanceof IpcUnavailableError
            ? { kind: 'outside-app' }
            : { kind: 'error', message: String(e) },
        ),
      );
  }, []);

  if (failure?.kind === 'outside-app') {
    return <Muted>{t('about.outsideApp')}</Muted>;
  }
  if (failure) {
    return <Muted>{t('about.coreError', { message: failure.message })}</Muted>;
  }
  if (!info) {
    return <Muted>{t('common.loading')}</Muted>;
  }
  return (
    <div>
      <h3 className="text-subtitle font-semibold">{info.name}</h3>
      <Muted>{t('about.version', { version: info.version })}</Muted>
    </div>
  );
}
