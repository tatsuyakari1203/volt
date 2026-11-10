import { invoke } from '@tauri-apps/api/core';

/** Name and version reported by the Rust shell. Mirrors `AppInfo` in volt-desktop. */
export interface AppInfo {
  name: string;
  version: string;
}

/** Asks the Rust side who it is. Proves the frontend-to-Rust path works. */
export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('app_info');
}
