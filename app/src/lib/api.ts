// Typed wrappers around Tauri commands. Rust command names are snake_case.
import { invoke } from '@tauri-apps/api/core'

export interface AppInfo {
  version: string
  schema_version: number
  data_dir: string
  db_path: string
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
}
