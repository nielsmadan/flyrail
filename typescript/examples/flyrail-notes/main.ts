import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  Bundle,
  DirectoryStore,
  Target,
  inspect,
  install,
  uninstall,
  update,
  type Installation,
  type Observation,
  type TargetError,
  type TargetInspection,
  type TargetResult,
} from 'flyrail';

export interface Output {
  write(text: string): unknown;
}

export interface Io {
  readonly stdout: Output;
  readonly stderr: Output;
  readonly signal?: AbortSignal;
}

const program = 'flyrail-notes';
const appVersion = '0.1.0';
const lockTimeoutMs = 5000;

const usage = `Usage:
  flyrail-notes --version
  flyrail-notes words FILE
  flyrail-notes ai status --source SOURCE --target TARGET [--target TARGET ...] --state STATE
  flyrail-notes ai install --source SOURCE --target TARGET [--target TARGET ...] --state STATE
  flyrail-notes ai update --source SOURCE --target TARGET [--target TARGET ...] --state STATE [--replace-modified]
  flyrail-notes ai uninstall --bundle-id ID --target TARGET [--target TARGET ...] --state STATE [--replace-modified]`;

type Action = 'status' | 'install' | 'update' | 'uninstall';

interface AiRequest {
  readonly action: Action;
  readonly source: string;
  readonly bundleId: string;
  readonly targets: readonly string[];
  readonly state: string;
  readonly replaceModified: boolean;
}

class UsageError extends Error {}

const actions: readonly string[] = ['status', 'install', 'update', 'uninstall'];
const valueOptions = new Set(['--source', '--bundle-id', '--target', '--state']);

const isAction = (value: string): value is Action => actions.includes(value);

const takeValue = (option: string, rest: readonly string[]): [string, readonly string[]] => {
  const [value, ...after] = rest;
  if (value === '--' && after.length > 0) {
    return [after[0] ?? '', after.slice(1)];
  }
  if (value === undefined || value.startsWith('--')) {
    throw new UsageError(`${option} requires a value`);
  }
  return [value, after];
};

const parseOptions = (args: readonly string[]): Map<string, string[]> => {
  const options = new Map<string, string[]>();
  let rest = args;
  while (rest.length > 0) {
    const [option = '', ...after] = rest;
    if (option === '--replace-modified') {
      options.set(option, ['true']);
      rest = after;
      continue;
    }
    if (!valueOptions.has(option)) {
      throw new UsageError(`unknown option '${option}'`);
    }
    const [value, remaining] = takeValue(option, after);
    options.set(option, [...(options.get(option) ?? []), value]);
    rest = remaining;
  }
  return options;
};

function parseAi(args: readonly string[]): AiRequest {
  const [action, ...rest] = args;
  if (action === undefined) {
    throw new UsageError('missing AI action');
  }
  if (!isAction(action)) {
    throw new UsageError(`unknown AI action '${action}'`);
  }
  const options = parseOptions(rest);
  const last = (name: string): string => options.get(name)?.at(-1) ?? '';
  const request: AiRequest = {
    action,
    source: last('--source'),
    bundleId: last('--bundle-id'),
    targets: options.get('--target') ?? [],
    state: last('--state'),
    replaceModified: options.has('--replace-modified'),
  };
  const removal = action === 'uninstall';
  const problems: readonly (readonly [boolean, string])[] = [
    [request.targets.length === 0, 'at least one --target is required'],
    [request.state === '', '--state is required'],
    [removal && request.source !== '', 'uninstall does not accept --source'],
    [removal && request.bundleId === '', 'uninstall requires --bundle-id'],
    [!removal && request.bundleId !== '', 'only uninstall accepts --bundle-id'],
    [!removal && request.source === '', `${action} requires --source`],
    [
      request.replaceModified && action !== 'update' && !removal,
      `${action} does not accept --replace-modified`,
    ],
  ];
  const problem = problems.find(([failed]) => failed);
  if (problem !== undefined) {
    throw new UsageError(problem[1]);
  }
  return request;
}

const errorReport = (error: TargetError | null) =>
  error === null
    ? null
    : { code: error.code, message: error.message, path: error.path, errno: error.errno };

const installationReport = (installation: Installation) => ({
  bundle_id: installation.bundleId,
  version: installation.version,
  content_digest: installation.contentDigest,
  transaction_id: installation.transactionId,
  entries: installation.entries.map((entry) => ({
    path: entry.path,
    size: entry.size === null ? null : Number(entry.size),
    sha256: entry.sha256,
    executable: entry.executable,
  })),
});

const observationReport = (observation: Observation) => ({
  state: observation.state,
  installed: observation.installed === null ? null : installationReport(observation.installed),
  installations: observation.installations.map(installationReport),
  version_matches: observation.versionMatches,
  recorded_content_matches: observation.recordedContentMatches,
  content_matches: observation.contentMatches,
  modifications: observation.modifications.map((item) => ({
    bundle_id: item.bundleId,
    path: item.path,
    kind: item.kind,
  })),
  conflicts: observation.conflicts.map((item) => ({ path: item.path, owner: item.owner })),
  error: errorReport(observation.error),
  recovery_paths: observation.recoveryPaths,
});

const targetReport = (target: Target) => ({
  root: target.root,
  agent: target.agent,
  scope: target.scope,
});

const inspectionReport = (item: TargetInspection) => ({
  target: targetReport(item.target),
  root: item.root,
  alias_of: item.aliasOf,
  observation: observationReport(item.observation),
  is_current: item.observation.isCurrent,
});

const resultReport = (item: TargetResult) => ({
  target: targetReport(item.target),
  root: item.root,
  alias_of: item.aliasOf,
  status: item.status,
  observation: observationReport(item.observation),
  error: errorReport(item.error),
  recovery_paths: item.recoveryPaths,
  is_current: item.observation.isCurrent,
});

const clean = (results: readonly TargetResult[]): boolean =>
  results.every(
    (result) =>
      result.succeeded &&
      result.error === null &&
      result.observation.error === null &&
      result.recoveryPaths.length === 0,
  );

const report = (bundleId: string, bundleVersion: string | null, results: readonly object[]) =>
  `${JSON.stringify({ app_version: appVersion, bundle_id: bundleId, bundle_version: bundleVersion, results })}\n`;

async function ai(request: AiRequest, io: Io): Promise<number> {
  const targets = request.targets.map((root) => Target.directory(resolve(root)));
  const store = await DirectoryStore.open(resolve(request.state));
  const lockOptions = { lockTimeoutMs, ...(io.signal === undefined ? {} : { signal: io.signal }) };
  const options = { ...lockOptions, replaceModified: request.replaceModified };
  if (request.action === 'uninstall') {
    const results = await uninstall(request.bundleId, targets, store, options);
    io.stdout.write(report(request.bundleId, null, results.map(resultReport)));
    return clean(results) ? 0 : 1;
  }
  const bundle = await Bundle.fromDirectory(resolve(request.source));
  if (request.action === 'status') {
    const inspections = await inspect(bundle, targets, store);
    io.stdout.write(report(bundle.id, bundle.version, inspections.map(inspectionReport)));
    return inspections.every((item) => item.observation.isCurrent) ? 0 : 1;
  }
  const results =
    request.action === 'install'
      ? await install(bundle, targets, store, lockOptions)
      : await update(bundle, targets, store, options);
  io.stdout.write(report(bundle.id, bundle.version, results.map(resultReport)));
  return clean(results) ? 0 : 1;
}

const countWords = async (path: string): Promise<number> => {
  const bytes = await readFile(path);
  let text: string;
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    throw new Error(`${path} is not valid UTF-8`);
  }
  return text.split(/\s+/u).filter((word) => word !== '').length;
};

const message = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

export async function run(argv: readonly string[], io: Io): Promise<number> {
  const [command, ...rest] = argv;
  try {
    if (command === '--version' && rest.length === 0) {
      io.stdout.write(`${program} ${appVersion}\n`);
      return 0;
    }
    if (command === 'words' && rest.length === 1) {
      io.stdout.write(`${String(await countWords(resolve(rest[0] ?? '')))}\n`);
      return 0;
    }
    if (command === 'ai') {
      return await ai(parseAi(rest), io);
    }
    throw new UsageError(
      command === undefined ? 'missing command' : `invalid command or arguments for '${command}'`,
    );
  } catch (error) {
    if (error instanceof UsageError) {
      io.stderr.write(`${program}: ${error.message}\n${usage}\n`);
      return 2;
    }
    if (io.signal?.aborted === true && error === io.signal.reason) {
      io.stderr.write(`${program}: interrupted before any change was committed\n`);
      return 1;
    }
    io.stderr.write(`${program}: ${message(error)}\n`);
    return 2;
  }
}
