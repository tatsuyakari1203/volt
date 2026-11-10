// The frontend's only door to the Rust shell. Commands, events and transfer types come from
// `bindings.ts`, which is generated from Rust (see docs/architecture.md): never hand-write a
// transfer type or call `invoke` directly. This file adds ergonomics on top of the generated
// module and nothing else.

import { t } from '../i18n/translate';
import {
  type ActivityExport,
  type ActivityPage,
  type AppInfo,
  type AppListView,
  type ApplyResult,
  type AppReady,
  type BoostConfigView,
  type BoostResult,
  type CleaningRuleInfo,
  type CleanRunResult,
  type CleanScanProgress,
  type CleanScanResult,
  type ClearResult,
  commands,
  type ElevatedCleanResult,
  type ElevationStatusView,
  type EmptyBinResult,
  type EndAppResult,
  type ExportResult,
  events,
  type HealthView,
  type HistoryStatus,
  type IpcError,
  type ItemPick,
  type LanguageChoice,
  type LanguageSetting,
  type LargeFilesProgress,
  type LargeFilesResult,
  type LargeFilesTrashResult,
  type MemoryAreaView,
  type MemoryCleanResult,
  type MemoryState,
  type MetricsView,
  type ProcessListView,
  type RecycleBinView,
  type Result,
  type RevertResult,
  type StartupChangeResult,
  type StartupList,
  type TweakInfo,
  type TweakPlanView,
  type UndoPreview,
  type UndoResult,
  type UninstallResult,
} from './bindings';

export type {
  ActivityExport,
  ActivityItem,
  ActivityPage,
  ActivitySource,
  ActivityStatus,
  AppInfo,
  AppListView,
  ApplyResult,
  ApplyStatus,
  AppProblemView,
  AppReady,
  AppSourceView,
  AppView,
  AreaResult,
  AreaStatus,
  ArgView,
  BoostConfigView,
  BoostResult,
  ChangeState,
  CleanCategoryView,
  CleaningRuleInfo,
  CleanRunResult,
  CleanScanProgress,
  CleanScanResult,
  ClearResult,
  DeductionView,
  DiskView,
  ElevatedCleanResult,
  ElevationReasonView,
  ElevationStatusView,
  EmptyBinResult,
  EndAppResult,
  EndabilityView,
  EndStatusView,
  EvidenceLevel,
  ExportResult,
  FileGroupView,
  FixActionView,
  FollowView,
  GpuView,
  HealthCheckView,
  HealthLevelView,
  HealthView,
  HiddenReasonView,
  IpcError,
  ItemPick,
  JunkView,
  Lang,
  LanguageChoice,
  LanguageSetting,
  LargeFilesProgress,
  LargeFilesResult,
  LargeFilesTrashResult,
  LargeFileView,
  MemoryAreaView,
  MemoryCleanResult,
  MemoryListsView,
  MemoryOperationView,
  MemoryState,
  MemoryView,
  MetricsView,
  MovedToBinView,
  OperationState,
  PhraseView,
  ProcessEndView,
  ProcessGroupView,
  ProcessListView,
  RecycleBinView,
  RestartKind,
  RevertResult,
  RevertStatus,
  RiskLevel,
  RuleCleanView,
  RuleScanView,
  ScanItemView,
  ScheduledTaskView,
  SkipGroup,
  SkippedOperationView,
  StartupChangeResult,
  StartupChangeStatus,
  StartupEntryView,
  StartupList,
  StartupProblemView,
  StartupSource,
  TweakCategory,
  TweakInfo,
  TweakPlanView,
  TweakStateView,
  TweakStepView,
  UndoFailure,
  UndoOverwrite,
  UndoPreview,
  UndoPreviewItem,
  UndoResult,
  UndoStatus,
  UninstallOutcomeView,
  UninstallResult,
} from './bindings';

/** Thrown by the wrappers below when a command returns an error; carries the typed value. */
export class IpcCommandError extends Error {
  /** The typed error the Rust command returned. Switch on `error.kind`. */
  readonly error: IpcError;

  constructor(error: IpcError) {
    super(describeIpcError(error));
    this.name = 'IpcCommandError';
    this.error = error;
  }
}

/** One-line text for an IPC error. The switch is exhaustive: a new Rust variant fails typecheck. */
export function describeIpcError(error: IpcError): string {
  switch (error.kind) {
    case 'notFound':
      return t('ipc.notFound', { message: error.message });
    case 'accessDenied':
      return t('ipc.accessDenied', { message: error.message });
    case 'unsupported':
      return t('ipc.unsupported', { message: error.message });
    case 'invalidInput':
      return t('ipc.invalidInput', { message: error.message });
    case 'timedOut':
      return t('ipc.timedOut', { message: error.message });
    case 'other':
      return error.message;
    default:
      return assertNever(error);
  }
}

/**
 * Thrown by the wrappers below when the page runs outside the Volt window (a plain browser, a
 * screenshot tool): there is no Tauri bridge to call. It is an expected situation, not a bug, so
 * screens can say so in plain words instead of showing a raw `TypeError`.
 */
export class IpcUnavailableError extends Error {
  constructor() {
    super('Not available outside the Volt app.');
    this.name = 'IpcUnavailableError';
  }
}

/** Whether the Tauri bridge exists, which is only true inside the Volt window. */
function insideVolt(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * Runs a generated command and unwraps its result. Outside the Volt window the generated
 * binding fails with a `TypeError` (the bridge is missing); that becomes an `IpcUnavailableError`.
 */
async function call<T>(run: () => Promise<Result<T, IpcError>>): Promise<T> {
  try {
    return unwrap(await run());
  } catch (error) {
    if (error instanceof TypeError && !insideVolt()) {
      throw new IpcUnavailableError();
    }
    throw error;
  }
}

function assertNever(value: never): never {
  throw new Error(`Unhandled IPC error: ${JSON.stringify(value)}`);
}

/** Returns the data of a command result, or throws an `IpcCommandError` carrying the error. */
export function unwrap<T>(result: Result<T, IpcError>): T {
  if (result.status === 'error') {
    throw new IpcCommandError(result.error);
  }
  return result.data;
}

/** Asks the Rust side who it is. Proves the frontend-to-Rust path works. */
export async function getAppInfo(): Promise<AppInfo> {
  return call(() => commands.appInfo());
}

/** Calls `handler` each time the shell announces that the app is ready. Returns the unlisten function. */
export function onAppReady(handler: (info: AppReady) => void): Promise<() => void> {
  return events.appReady.listen((event) => handler(event.payload));
}

/** Whether Volt is registered to start with Windows (per-user, unelevated). */
export async function getAutostartEnabled(): Promise<boolean> {
  return call(() => commands.autostartEnabled());
}

/** Turns start with Windows on or off. Resolves with the state that is true afterwards. */
export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  return call(() => commands.setAutostart(enabled));
}

/**
 * Calls `handler` with the report each time Boost was run from the tray menu (the shell ran it
 * already, window open or not). Returns the unlisten function.
 */
export function onBoostFinished(handler: (report: BoostResult) => void): Promise<() => void> {
  return events.boostFinished.listen((event) => handler(event.payload));
}

/**
 * Runs Boost as configured and resolves with the before and after report: available memory, the
 * memory operations that ran or were skipped (with the reason), what the read-only scan found and,
 * only when the user opted in, what was moved to the Recycle Bin. There is no "freed" figure.
 * It cannot be undone; the shell records it in the activity history.
 */
export async function runBoost(): Promise<BoostResult> {
  return call(() => commands.boost());
}

/** What Boost includes (the defaults until the user changes them). */
export async function getBoostConfig(): Promise<BoostConfigView> {
  return call(() => commands.boostConfig());
}

/** Saves what Boost includes and resolves with what is stored. */
export async function setBoostConfig(config: BoostConfigView): Promise<BoostConfigView> {
  return call(() => commands.setBoostConfig(config));
}

/** Every tweak of the catalogue with its state read live from the system. */
export async function listTweaks(): Promise<TweakInfo[]> {
  return call(() => commands.listTweaks());
}

/** Applies a tweak. Resolves with how it ended; a failed step is a result, not a rejection. */
export async function applyTweak(id: string): Promise<ApplyResult> {
  return call(() => commands.applyTweak(id));
}

/** Reverts what Volt applied for a tweak. Resolves with whether the revert was complete. */
export async function revertTweak(id: string): Promise<RevertResult> {
  return call(() => commands.revertTweak(id));
}

/** What a tweak changes (every step) and what turning it off would write without a journal record. */
export async function getTweakPlan(id: string): Promise<TweakPlanView> {
  return call(() => commands.getTweakPlan(id));
}

/**
 * Turns off a tweak Volt did not apply, by removing the registry values it sets so Windows uses
 * its default. Journaled, so it can be undone from the Undo page.
 */
export async function resetTweak(id: string): Promise<RevertResult> {
  return call(() => commands.resetTweak(id));
}

/**
 * Reads the activity history, newest first. Pass the previous page's `nextCursor` as `cursor`
 * to read the next, older page.
 */
export async function listActivity(limit: number, cursor?: string | null): Promise<ActivityPage> {
  return call(() => commands.listActivity(limit, cursor ?? null));
}

/**
 * Everything that starts with Windows (Run keys and Startup folders), each with whether it is
 * enabled. Sources that could not be read come back in `problems`; the rest of the list is valid.
 */
export async function listStartupEntries(): Promise<StartupList> {
  return call(() => commands.listStartupEntries());
}

/**
 * Switches a startup entry on or off the way Windows does, through the undo journal. Machine-wide
 * entries reject with an `accessDenied` error (administrator rights).
 */
export async function setStartupEntryEnabled(
  id: string,
  enabled: boolean,
): Promise<StartupChangeResult> {
  return call(() => commands.setStartupEntryEnabled(id, enabled));
}

/**
 * Calls `handler` with a fresh reading of the machine (CPU, memory, disks, GPU) about once a
 * second while the window is open. Returns the unlisten function. GPU fields are `null` on a
 * machine without a readable GPU, and `cpuPercent` can be `null`: show a dash, not an error.
 */
export function onMetricsUpdated(handler: (metrics: MetricsView) => void): Promise<() => void> {
  return events.metricsUpdated.listen((event) => handler(event.payload));
}

/**
 * The latest reading, or `null` before the first one of this window. Call it once after
 * `onMetricsUpdated` so a screen shows numbers at once instead of waiting for the next second.
 */
export async function getLatestMetrics(): Promise<MetricsView | null> {
  return call(() => commands.latestMetrics());
}

/**
 * Checks the machine now and resolves with the health score and every deduction behind it
 * (read-only). The score is exactly 100 minus the points of the deductions. A check that could
 * not run is listed in `unchecked`; it is not an error. Call it when Home opens and when the
 * user asks to check again, not on a timer: memory pressure is volatile.
 */
export async function getHealthReport(): Promise<HealthView> {
  return call(() => commands.healthReport());
}

/**
 * Undoes one change: `entry` is the `entry` of an activity row. Resolves with how it ended
 * (restored, failed or nothing to undo); a failed restore is a result, not a rejection.
 * `acknowledged` holds the `laterEntry` ids of the later changes the user was shown (from
 * `previewUndoEntry`); an undo that would overwrite another one resolves with
 * `needsAcknowledgement` and changes nothing.
 */
export async function undoEntry(entry: string, acknowledged: string[] = []): Promise<UndoResult> {
  return call(() => commands.undoEntry(entry, acknowledged));
}

/**
 * Undoes every change of a batch (the `batch` of an activity row) that is still applied. Resolves
 * with how it ended (restored, partly restored with what failed, failed or nothing to undo).
 * `acknowledged` works as for `undoEntry`.
 */
export async function undoBatch(batch: string, acknowledged: string[] = []): Promise<UndoResult> {
  return call(() => commands.undoBatch(batch, acknowledged));
}

/** What undoing a batch would restore right now, newest first. Reads only. */
export async function previewUndoBatch(batch: string): Promise<UndoPreview> {
  return call(() => commands.previewUndoBatch(batch));
}

/**
 * What undoing one change would do right now: the change, and the later change of the same
 * target it would overwrite, if any. Reads only.
 */
export async function previewUndoEntry(entry: string): Promise<UndoPreview> {
  return call(() => commands.previewUndoEntry(entry));
}

/** Clears the activity history; changes that can still be undone stay listed. */
export async function clearHistory(): Promise<ClearResult> {
  return call(() => commands.clearHistory());
}

/** Whether the activity history is complete since the app started. */
export async function historyStatus(): Promise<HistoryStatus> {
  return call(() => commands.historyStatus());
}

/**
 * Asks the user where to save the activity log (a native save dialog opened by the Rust side) and
 * writes it as a text file. Resolves with `cancelled` when the dialog was closed.
 */
export async function exportActivity(): Promise<ExportResult> {
  return call(() => commands.exportActivity());
}

/** The whole activity log as text, for the copy-to-clipboard fallback. */
export async function exportActivityText(): Promise<ActivityExport> {
  return call(() => commands.exportActivityText());
}

/** The cleaning rules, without touching the file system. */
export async function listCleaningRules(): Promise<CleaningRuleInfo[]> {
  return call(() => commands.listCleaningRules());
}

/**
 * Scans every cleaning rule (read-only) and resolves with counts and sizes per rule and the id of
 * the scan, which `cleanSelectedRules` needs. Runs in the background: listen with
 * `onCleanScanProgress` for how far it is and call `cancelCleaningScan` to stop it, even inside a
 * rule (the result then has `cancelled: true` and lists the rules it reached, the last of them
 * possibly `truncated`; with no rule reached the earlier scan stands). Results carry rule-relative sample paths, never a full
 * path.
 */
export async function scanCleaning(): Promise<CleanScanResult> {
  return call(() => commands.scanCleaning());
}

/**
 * Scans only the chosen rules (read-only), like `scanCleaning`: how to continue with the rest of a
 * rule whose scan was truncated. Resolves with a new scan id; nothing moves until it is cleaned.
 */
export async function scanCleaningRules(ruleIds: readonly string[]): Promise<CleanScanResult> {
  return call(() => commands.scanCleaningRules([...ruleIds]));
}

/** Asks the running scan to stop, inside the rule it is walking or before the next. */
export async function cancelCleaningScan(): Promise<void> {
  await call(() => commands.cancelCleaningScan());
}

/**
 * Calls `handler` before each rule of a scan is looked at (`done` of `total` rules are finished).
 * Returns the unlisten function.
 */
export function onCleanScanProgress(
  handler: (progress: CleanScanProgress) => void,
): Promise<() => void> {
  return events.cleanScanProgress.listen((event) => handler(event.payload));
}

/**
 * Moves the files of the chosen rules of scan `planId` to the Recycle Bin. A scan can be cleaned
 * once: a newer scan or an earlier clean makes the id `notFound`. For a rule picked item by item
 * (`pickItems`), `picks` names the items to move; none picked is `invalidInput`. Rules that need administrator
 * rights are `accessDenied`. The result says what was moved and what was left, with reasons; it
 * has no "freed" figure because nothing is freed until the Recycle Bin is emptied.
 */
export async function cleanSelectedRules(
  planId: string,
  ruleIds: string[],
  picks: ItemPick[] = [],
): Promise<CleanRunResult> {
  return call(() => commands.cleanSelectedRules(planId, ruleIds, picks));
}

/**
 * Adds a folder for a rule that needs one (a ComfyUI install, for its temp folder) and keeps it.
 * Nothing in the folder is read or changed; scan again afterwards. A folder that is too broad, a
 * protected folder, an output folder or not a plain folder is `invalidInput`.
 */
export async function chooseCleaningFolder(ruleId: string, folder: string): Promise<void> {
  await call(() => commands.chooseCleaningFolder(ruleId, folder));
}

/** Forgets the folder added for a rule. The folder itself is never touched. */
export async function forgetCleaningFolder(ruleId: string): Promise<void> {
  await call(() => commands.forgetCleaningFolder(ruleId));
}

/** How many items the Recycle Bin holds and how big they are. Read-only. */
export async function getRecycleBinStatus(): Promise<RecycleBinView> {
  return call(() => commands.recycleBinStatus());
}

/** Permanently deletes everything in the Recycle Bin. Cannot be undone: confirm with the user first. */
export async function emptyRecycleBin(): Promise<EmptyBinResult> {
  return call(() => commands.emptyRecycleBin());
}

/**
 * Scans a folder for its biggest files (the system drive when `root` is `null`), read-only. Runs in
 * the background: listen with `onLargeFilesProgress` and call `cancelLargeFilesScan` to stop it
 * (the result then has `cancelled: true`). The result names the scan, which the calls below need.
 */
export async function scanLargeFiles(root: string | null): Promise<LargeFilesResult> {
  return call(() => commands.scanLargeFiles(root));
}

/** Asks the running large files scan to stop. Does nothing when none runs. */
export async function cancelLargeFilesScan(): Promise<void> {
  await call(() => commands.cancelLargeFilesScan());
}

/** Calls `handler` every so often while a large files scan runs. Returns the unlisten function. */
export function onLargeFilesProgress(
  handler: (progress: LargeFilesProgress) => void,
): Promise<() => void> {
  return events.largeFilesProgress.listen((event) => handler(event.payload));
}

/**
 * Moves the files at the given rows of scan `scanId` to the Recycle Bin (never a permanent
 * delete). The result says what moved, what was left, and the list that remains.
 */
export async function trashLargeFiles(
  scanId: string,
  indices: number[],
): Promise<LargeFilesTrashResult> {
  return call(() => commands.trashLargeFiles(scanId, indices));
}

/** Opens the folder of the file at `index` of scan `scanId` in Explorer, file selected. */
export async function revealLargeFile(scanId: string, index: number): Promise<void> {
  await call(() => commands.revealLargeFile(scanId, index));
}

/** Shows the native folder dialog. The chosen folder, or `null` when it was closed. */
export async function pickScanFolder(): Promise<string | null> {
  return call(() => commands.pickScanFolder());
}

/**
 * Memory by list (in use, standby, modified, free) and which cleaning operations can run now.
 * Reads only. An operation that needs administrator rights comes back with its reason.
 */
export async function getMemoryState(): Promise<MemoryState> {
  return call(() => commands.memoryState());
}

/**
 * Runs the selected memory operations and resolves with memory by list before and after and how
 * each operation ended. An operation that could not run is a result with a status, not a
 * rejection. It cannot be undone; the shell records it in the activity history.
 */
export async function cleanMemory(areas: MemoryAreaView[]): Promise<MemoryCleanResult> {
  return call(() => commands.cleanMemory(areas));
}

/**
 * Whether administrator actions can be used now, and if not, why. Reads only: it never starts the
 * helper and never shows a prompt.
 */
export async function getElevationStatus(): Promise<ElevationStatusView> {
  return call(() => commands.elevationStatus());
}

/**
 * Runs the selected memory operations as administrator through the helper. The first call of a
 * session shows one administrator prompt; cancelling it resolves with `outcome: 'declined'` and
 * nothing is changed or recorded. Otherwise the report is the same as `cleanMemory`'s.
 */
export async function cleanMemoryElevated(areas: MemoryAreaView[]): Promise<ElevatedCleanResult> {
  return call(() => commands.cleanMemoryElevated(areas));
}

/**
 * The running processes grouped by app (one entry per executable) with memory, CPU share and
 * whether each may be ended. Reads only. CPU share compares with the previous call, so it is
 * `null` on the first one: poll at a steady pace.
 */
export async function listProcesses(): Promise<ProcessListView> {
  return call(() => commands.listProcesses());
}

/**
 * Ends every process of an app by its `id`. Cannot be undone and loses unsaved work: confirm with
 * the user first. Windows-critical apps and Volt are refused by the core (`invalidInput`). A
 * process that needs administrator rights is a status in the result, not a rejection.
 */
export async function endApp(id: string): Promise<EndAppResult> {
  return call(() => commands.endApp(id));
}

/** The saved language choice and the display language of Windows, which `System` stands for. */
export async function getLanguageSetting(): Promise<LanguageSetting> {
  return call(() => commands.languageSetting());
}

/** Saves the language choice. The shell also rewrites the tray menu in the new language at once. */
export async function saveLanguageSetting(choice: LanguageChoice): Promise<LanguageSetting> {
  return call(() => commands.setLanguage(choice));
}

/**
 * Every installed app: desktop apps from the registry and the packaged apps of the current user,
 * with size, publisher and install date where known. Entries that are system components, updates,
 * frameworks or cannot be removed are included with a hidden reason: the screen hides them by
 * default. Sources that could not be read come back in problems. Read-only.
 */
export async function listApps(): Promise<AppListView> {
  return call(() => commands.listApps());
}

/**
 * Removes one app by running its own registered uninstaller (or asking Windows to remove a
 * packaged app) and waits until it exits. The result says whether the app is gone, still installed
 * or could not be started, with the list as it is now. Volt cannot undo it. The id comes from
 * listApps; a command is never passed. System components and updates are refused unless
 * `includeSystem` is set, which the screen does only while "Show system components" is on.
 */
export async function uninstallApp(id: string, includeSystem: boolean): Promise<UninstallResult> {
  return call(() => commands.uninstallApp(id, includeSystem));
}
