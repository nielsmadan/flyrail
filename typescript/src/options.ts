export interface CallOptions {
  readonly signal?: AbortSignal;
}

export interface LockOptions extends CallOptions {
  readonly lockTimeoutMs?: number;
}
