import { Recycle, ShieldAlert } from 'lucide-react';
import { useEffect, useState } from 'react';
import { t } from '../../shared/i18n';
import {
  type BoostConfigView,
  getBoostConfig,
  getMemoryState,
  type MemoryOperationView,
  setBoostConfig,
} from '../../shared/ipc';
import { describeError, operationText } from '../../shared/text/memory';
import { Badge, Card, ListGroup, ListRow, Muted, Switch } from '../../shared/ui';

/**
 * Settings card for what Boost includes: which memory operations run, and the opt-in to move the
 * temporary files to the Recycle Bin (which skips the preview the Clean page gives). Operations
 * that need administrator rights are listed and cannot be switched on: Boost would only skip them.
 */
export function BoostSettings() {
  const [config, setConfig] = useState<BoostConfigView | null>(null);
  const [operations, setOperations] = useState<MemoryOperationView[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    getBoostConfig()
      .then((value) => {
        if (alive) setConfig(value);
      })
      .catch((e: unknown) => {
        if (alive) setError(t('settings.boost.readError', { message: describeError(e) }));
      });
    getMemoryState()
      .then((state) => {
        if (alive) setOperations(state.operations);
      })
      .catch(() => {
        // Without availability the rows are not listed; the settings card still shows the opt-in.
      });
    return () => {
      alive = false;
    };
  }, []);

  function save(next: BoostConfigView) {
    setBusy(true);
    setError(null);
    setBoostConfig(next)
      .then(setConfig)
      .catch((e: unknown) => setError(t('settings.boost.saveError', { message: describeError(e) })))
      .finally(() => setBusy(false));
  }

  function toggleArea(area: MemoryOperationView['area'], on: boolean) {
    if (!config) return;
    const without = config.areas.filter((candidate) => candidate !== area);
    save({ ...config, areas: on ? [...without, area] : without });
  }

  return (
    <Card as="section" aria-labelledby="settings-boost" padding="none">
      <h2 id="settings-boost" className="px-4 pt-4 pb-1 text-subtitle font-semibold">
        {t('settings.boost.title')}
      </h2>
      <p className="px-4 pb-3 text-caption text-muted">{t('settings.boost.intro')}</p>
      <ListGroup
        aria-label={t('settings.boost.operations')}
        className="rounded-none border-x-0 border-b-0 shadow-none"
      >
        {operations.map((operation) => {
          const text = operationText(operation.area);
          const available = operation.state === 'available';
          return (
            <ListRow
              key={operation.area}
              title={text.title}
              badge={
                available ? null : (
                  <Badge>
                    <ShieldAlert aria-hidden size={12} />
                    {operation.state === 'needsAdministrator'
                      ? t('common.needsAdmin')
                      : t('common.notSupported')}
                  </Badge>
                )
              }
              description={text.description}
              trailing={
                <Switch
                  aria-label={text.title}
                  checked={available && config?.areas.includes(operation.area) === true}
                  disabled={config === null || busy || !available}
                  onCheckedChange={(on) => toggleArea(operation.area, on)}
                />
              }
            />
          );
        })}
        <ListRow
          icon={<Recycle aria-hidden size={18} />}
          title={t('settings.boost.temp.title')}
          description={t('settings.boost.temp.description')}
          trailing={
            <Switch
              aria-label={t('settings.boost.temp.title')}
              checked={config?.moveTempToRecycleBin === true}
              disabled={config === null || busy}
              onCheckedChange={(on) => {
                if (config) save({ ...config, moveTempToRecycleBin: on });
              }}
            />
          }
        />
      </ListGroup>
      {error ? (
        <div role="alert" className="px-4 py-3">
          <Muted>{error}</Muted>
        </div>
      ) : null}
    </Card>
  );
}
