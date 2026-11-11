import { Eye, Layers, Loader2, Plus } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { localeTag, type MessageKey, t, tn } from '../../shared/i18n';
import {
  type AutomationConfigView,
  getAutomationStatus,
  listProfiles,
  listTweaks,
  onProfilesUpdated,
  type ProfilesView,
  type ProfileView,
  type TweakInfo,
} from '../../shared/ipc';
import { Badge, Button, Card, ListGroup, ListRow, PageHeader } from '../../shared/ui';
import { EditDialog } from './EditDialog';
import { automationSummary, describeError } from './messages';
import { SwitchDialog, type SwitchTarget } from './SwitchDialog';

type Load =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; view: ProfilesView };

const reasonKeys = {
  fullscreen: 'settings.automation.reason.fullscreen',
  batterySaver: 'settings.automation.reason.batterySaver',
  manualRun: 'settings.automation.reason.manualRun',
} as const satisfies Record<string, MessageKey>;

function when(ms: number): string {
  return new Date(ms).toLocaleString(localeTag(), { dateStyle: 'medium', timeStyle: 'short' });
}

/**
 * Profiles: named sets of tweaks and automation settings (Gaming, Work, AI workload, Battery to
 * start from). None is active until the user chooses one. Switching shows exactly what changes
 * first, and a profile can name an app that activates it. All rules live in the Rust core.
 */
export function ProfilesScreen() {
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [tweaks, setTweaks] = useState<TweakInfo[]>([]);
  const [automation, setAutomation] = useState<AutomationConfigView | null>(null);
  const [switching, setSwitching] = useState<SwitchTarget | null>(null);
  const [editing, setEditing] = useState<{ profile: ProfileView | null } | null>(null);

  const refresh = useCallback(async () => {
    try {
      setLoad({ status: 'ready', view: await listProfiles() });
    } catch (error) {
      setLoad({
        status: 'error',
        message: t('profiles.loadFailed', { message: describeError(error) }),
      });
    }
  }, []);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | null = null;
    void refresh();
    listTweaks()
      .then((list) => {
        if (alive) setTweaks(list);
      })
      .catch(() => {
        // Without the list the editor offers no tweaks to choose from.
      });
    getAutomationStatus()
      .then((status) => {
        if (alive) setAutomation(status.config);
      })
      .catch(() => {
        // Without it the editor cannot turn an automation part on or off.
      });
    onProfilesUpdated((view) => {
      if (alive) setLoad({ status: 'ready', view });
    })
      .then((stop) => {
        if (alive) unlisten = stop;
        else stop();
      })
      .catch(() => {
        // Not running inside the app: nothing is pushed.
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [refresh]);

  const view = load.status === 'ready' ? load.view : null;
  const active = view?.profiles.find((profile) => profile.id === view.active) ?? null;
  const watching = view?.profiles.some((profile) => profile.trigger !== null) ?? false;

  return (
    <>
      <PageHeader
        title={t('nav.profiles')}
        description={t('profiles.description')}
        actions={
          view ? (
            <Button
              size="sm"
              icon={<Plus aria-hidden size={16} />}
              onClick={() => setEditing({ profile: null })}
            >
              {t('profiles.new')}
            </Button>
          ) : null
        }
      />
      <div className="flex flex-col gap-3">
        {load.status === 'loading' ? (
          <Card role="status" className="flex items-center justify-center gap-2 py-10 text-muted">
            <Loader2 aria-hidden size={18} className="animate-spin" />
            {t('common.loading')}
          </Card>
        ) : null}
        {load.status === 'error' ? (
          <Card role="alert" className="flex flex-col items-center gap-3 py-10 text-center">
            <p className="text-muted">{load.message}</p>
            <Button onClick={() => void refresh()}>{t('common.tryAgain')}</Button>
          </Card>
        ) : null}
        {view ? (
          <>
            <Card className="flex flex-wrap items-center justify-between gap-3">
              <div className="min-w-0">
                <p className="font-semibold">
                  {active
                    ? t('profiles.active.is', { name: active.name })
                    : t('profiles.active.none')}
                </p>
                {view.suggested ? (
                  <p className="text-caption text-muted">{t('profiles.suggested')}</p>
                ) : null}
              </div>
              {active ? (
                <Button size="sm" onClick={() => setSwitching({ id: null, name: active.name })}>
                  {t('profiles.turnOff')}
                </Button>
              ) : null}
            </Card>

            {view.profiles.length === 0 ? (
              <Card className="flex flex-col items-center gap-2 py-8 text-center text-muted">
                {t('profiles.empty')}
              </Card>
            ) : (
              <ListGroup aria-label={t('nav.profiles')}>
                {view.profiles.map((profile) => (
                  <ProfileRow
                    key={profile.id}
                    profile={profile}
                    active={profile.id === view.active}
                    onSwitch={() => setSwitching({ id: profile.id, name: profile.name })}
                    onEdit={() => setEditing({ profile })}
                  />
                ))}
              </ListGroup>
            )}

            {watching ? (
              <Card as="section" aria-label={t('profiles.watch.title')} className="flex gap-3">
                <Eye aria-hidden size={18} className="mt-0.5 shrink-0 text-muted" />
                <div className="min-w-0">
                  <h2 className="font-semibold">{t('profiles.watch.title')}</h2>
                  <p className="text-body text-muted">
                    {t('profiles.watch.note', { seconds: view.pollSeconds })}
                  </p>
                  {view.lastSkip ? (
                    <p className="mt-1 text-caption text-muted">
                      {t('profiles.watch.skipped', {
                        when: when(view.lastSkip.at),
                        reason: t(reasonKeys[view.lastSkip.reason]),
                      })}
                    </p>
                  ) : null}
                </div>
              </Card>
            ) : null}
          </>
        ) : null}
      </div>

      {switching ? (
        <SwitchDialog
          target={switching}
          onClose={() => setSwitching(null)}
          onSwitched={() => void refresh()}
        />
      ) : null}
      {editing && view ? (
        <EditDialog
          profile={editing.profile}
          tweaks={tweaks}
          automation={automation}
          pollSeconds={view.pollSeconds}
          maxNameChars={view.maxNameChars}
          onClose={() => setEditing(null)}
          onChanged={(next) => {
            setLoad({ status: 'ready', view: next });
            setEditing(null);
          }}
        />
      ) : null}
    </>
  );
}

function ProfileRow({
  profile,
  active,
  onSwitch,
  onEdit,
}: {
  profile: ProfileView;
  active: boolean;
  onSwitch: () => void;
  onEdit: () => void;
}) {
  const automation = automationSummary(profile);
  const lines = [
    tn('profiles.row.tweaks', profile.tweaks.length),
    automation ? t('profiles.row.automation', { summary: automation }) : null,
    profile.trigger
      ? t(profile.trigger.returnOnExit ? 'profiles.row.triggerBack' : 'profiles.row.trigger', {
          exe: profile.trigger.exe,
        })
      : null,
  ].filter((line): line is string => line !== null);
  return (
    <ListRow
      icon={<Layers size={18} />}
      title={profile.name}
      badge={active ? <Badge>{t('profiles.row.active')}</Badge> : null}
      description={lines.join(' · ')}
      trailing={
        <span className="flex gap-2">
          <Button
            size="sm"
            aria-label={t('profiles.row.edit.label', { name: profile.name })}
            onClick={onEdit}
          >
            {t('profiles.row.edit')}
          </Button>
          <Button
            size="sm"
            variant="primary"
            disabled={active}
            aria-label={t('profiles.row.switch.label', { name: profile.name })}
            onClick={onSwitch}
          >
            {t('profiles.row.switch')}
          </Button>
        </span>
      }
    />
  );
}
