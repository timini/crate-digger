// Typed wrappers around Tauri commands. Rust command names are snake_case;
// Tauri converts camelCase argument names to snake_case automatically.
import { invoke } from '@tauri-apps/api/core'

export interface AppInfo {
  version: string
  schema_version: number
  data_dir: string
  db_path: string
}

export type JobState = 'queued' | 'running' | 'paused' | 'blocked' | 'failed' | 'cancelled' | 'done'
export type HoldCode = 'user' | 'quit' | 'connector_auth' | 'daily_limit' | 'storage_limit'

export interface Job {
  id: string
  kind: string
  connector: string | null
  state: JobState
  hold_code: HoldCode | null
  reason: string | null
  payload: unknown
  attempts: number
  max_attempts: number
  next_run_at: number
  created_at: number
  updated_at: number
}

export interface JobCount {
  kind: string
  state: JobState
  count: number
}

export interface SchedulerStatus {
  accepting: boolean
  playback_active: boolean
  staging_used_bytes: number
  staging_budget_bytes: number
  daily: { kind: string; used: number; limit: number }[]
}

export interface ConnectorHealth {
  connector: string
  status: 'ok' | 'auth_failed' | 'unavailable'
  reason: string | null
  updated_at: number
}

export interface Activity {
  jobs: Job[]
  counts: JobCount[]
  scheduler: SchedulerStatus
  connectors: ConnectorHealth[]
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  activity: (states: JobState[], limit = 200) => invoke<Activity>('activity', { states, limit }),
  pauseAll: () => invoke<number>('jobs_pause_all'),
  resumeAll: () => invoke<number>('jobs_resume_all'),
  cancelJob: (id: string) => invoke<void>('job_cancel', { id }),
  retryJob: (id: string) => invoke<void>('job_retry', { id }),
}
