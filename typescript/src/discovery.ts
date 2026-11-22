import { posix } from 'node:path';

import type { Agent } from './enums.js';
import { ValueError } from './errors.js';
import { runReadOnly } from './internal/abort.js';
import type { CallOptions } from './options.js';
import { awaitNative } from './internal/call.js';
import { optionalAbsolute, requireAbsolute } from './internal/paths.js';
import { native } from './native/loader.js';

export interface AgentPresence {
  readonly agent: Agent;
  readonly executable: string | null;
  readonly application: string | null;
}

export interface DetectAgentsOptions extends CallOptions {
  readonly searchPath: readonly string[];
  readonly home?: string | null;
  readonly applications?: readonly string[] | null;
  readonly noApplications?: boolean;
}

const searchEntry = (entry: string): string => {
  if (entry === '' || entry.includes(':')) {
    throw new ValueError('search path entries must be non-empty and must not contain ":"');
  }
  return requireAbsolute(entry, 'search path entry');
};

const applicationDirectories = (options: DetectAgentsOptions): string[] => {
  const home = optionalAbsolute(options.home, 'home');
  const explicit = options.applications !== undefined && options.applications !== null;
  if (options.noApplications === true && explicit) {
    throw new ValueError('applications and noApplications cannot be combined');
  }
  if (options.noApplications === true) {
    return [];
  }
  if (options.applications !== undefined && options.applications !== null) {
    return options.applications.map((entry) => requireAbsolute(entry, 'applications entry'));
  }
  return home === null ? ['/Applications'] : ['/Applications', posix.join(home, 'Applications')];
};

export function detectAgents(options: DetectAgentsOptions): Promise<readonly AgentPresence[]> {
  return runReadOnly(options.signal, async () => {
    if (!Array.isArray(options.searchPath)) {
      throw new ValueError('searchPath must be an array of absolute directories');
    }
    const searchPath = options.searchPath.map(searchEntry).join(':');
    const applications = applicationDirectories(options);
    const wire = await awaitNative(native.discoveryDetectAgents(searchPath, applications));
    return Object.freeze(
      wire.items.map((item) =>
        Object.freeze({
          agent: item.agent,
          executable: item.executable,
          application: item.application,
        }),
      ),
    );
  });
}
