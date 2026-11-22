import { EnvRef, type EnvironmentValue } from './artifacts.js';
import type { HookEvent, HookOutcome } from './enums.js';
import { ValueError } from './errors.js';
import { callNative } from './internal/call.js';
import { fromUnits, requireText, toUnits } from './internal/text.js';
import type {
  NativeHookConfig,
  NativeHookEffects,
  NativeHookHandler,
  NativeHookJson,
  NativeHookRequest,
  NativeHookResponse,
} from './native/binding.js';
import { native } from './native/loader.js';

const limits = native.hookLimits();
export const HOOK_INPUT_LIMIT: number = limits.input;
export const HOOK_OUTPUT_LIMIT: number = limits.output;
export const HOOK_ERROR_LIMIT: number = limits.error;
export const HOOK_TEXT_LIMIT: number = limits.text;
export const HOOK_JSON_DEPTH_LIMIT: number = limits.jsonDepth;
export const HOOK_HANDLER_LIMIT: number = limits.handler;
export const HOOK_EVENT_TIMEOUT_LIMIT_MS: number = limits.eventTimeoutMs;

const requireData = (data: Uint8Array): Uint8Array => {
  if (!(data instanceof Uint8Array)) {
    throw new ValueError('hook data must be a Uint8Array');
  }
  return data;
};

interface Registry<T> {
  readonly attach: (owner: object, handle: T) => void;
  readonly of: (owner: unknown) => T;
}

const registry = <T>(label: string): Registry<T> => {
  const handles = new WeakMap<object, T>();
  return {
    attach: (owner, handle) => {
      handles.set(owner, handle);
    },
    of: (owner) => {
      const handle = typeof owner === 'object' && owner !== null ? handles.get(owner) : undefined;
      if (handle === undefined) {
        throw new ValueError(`${label} has the wrong type`);
      }
      return handle;
    },
  };
};

const jsonHandles = registry<NativeHookJson>('hook JSON');
const requestHandles = registry<NativeHookRequest>('hook request');
const responseHandles = registry<NativeHookResponse>('hook response');
const effectsHandles = registry<NativeHookEffects>('hook effects');
const handlerHandles = registry<NativeHookHandler>('hook handler');
const configHandles = registry<NativeHookConfig>('hook config');

let adoptableJson: NativeHookJson | null = null;

export class HookJson {
  readonly isObject: boolean;
  readonly text: string | null;

  private constructor() {
    const handle = adoptableJson;
    adoptableJson = null;
    if (handle === null) {
      throw new ValueError('HookJson has no public constructor');
    }
    const units = handle.text();
    this.isObject = handle.isObject();
    this.text = units === null ? null : fromUnits(units);
    jsonHandles.attach(this, handle);
    Object.freeze(this);
  }

  static parse(data: Uint8Array): HookJson {
    return adoptJson(callNative(() => native.hookJsonParse(requireData(data))));
  }

  static object(members: readonly (readonly [string, HookJson])[]): HookJson {
    const keys = members.map(([key]) => requireText(key, 'hook object key'));
    const values = members.map(([, value]) => jsonHandle(value));
    return adoptJson(callNative(() => native.hookJsonObject(keys, values)));
  }

  static string(text: string): HookJson {
    return adoptJson(callNative(() => native.hookJsonString(toUnits(text))));
  }

  static boolean(value: boolean): HookJson {
    return adoptJson(callNative(() => native.hookJsonBoolean(value)));
  }

  static integer(value: bigint): HookJson {
    if (typeof value !== 'bigint') {
      throw new ValueError('hook integers must be bigint');
    }
    return adoptJson(callNative(() => native.hookJsonInteger(value)));
  }

  static array(items: readonly HookJson[]): HookJson {
    const handles = items.map(jsonHandle);
    return adoptJson(callNative(() => native.hookJsonArray(handles)));
  }

  static null(): HookJson {
    return adoptJson(callNative(() => native.hookJsonNull()));
  }

  bytes(): Uint8Array {
    return jsonHandle(this).bytes();
  }
}

function adoptJson(handle: NativeHookJson): HookJson {
  adoptableJson = handle;
  try {
    return Reflect.construct(HookJson, []) as HookJson;
  } finally {
    adoptableJson = null;
  }
}

const jsonHandle = (value: HookJson): NativeHookJson => jsonHandles.of(value);

const optionalJson = (handle: NativeHookJson | null): HookJson | null =>
  handle === null ? null : adoptJson(handle);

export interface HookRequestInit {
  readonly event: HookEvent;
  readonly context: HookJson;
  readonly tool?: string | null;
  readonly input?: HookJson | null;
  readonly result?: HookJson | null;
}

let adoptableRequest: NativeHookRequest | null = null;

export class HookRequest {
  readonly event: HookEvent;
  readonly context: HookJson;
  readonly tool: string | null;
  readonly input: HookJson | null;
  readonly result: HookJson | null;

  constructor(init: HookRequestInit) {
    const adopted = adoptableRequest;
    adoptableRequest = null;
    const handle: NativeHookRequest =
      adopted ??
      callNative(() =>
        native.hookRequestNew(
          init.event,
          jsonHandle(init.context),
          init.tool === null || init.tool === undefined ? undefined : toUnits(init.tool),
          init.input === null || init.input === undefined ? undefined : jsonHandle(init.input),
          init.result === null || init.result === undefined ? undefined : jsonHandle(init.result),
        ),
      );
    const tool = handle.tool();
    this.event = handle.event() as HookEvent;
    this.context = adoptJson(handle.context());
    this.tool = tool === null ? null : fromUnits(tool);
    this.input = optionalJson(handle.input());
    this.result = optionalJson(handle.result());
    requestHandles.attach(this, handle);
    Object.freeze(this);
  }

  static parse(data: Uint8Array): HookRequest {
    const handle = callNative(() => native.hookRequestParse(requireData(data)));
    adoptableRequest = handle;
    try {
      return new HookRequest(undefined as never);
    } finally {
      adoptableRequest = null;
    }
  }

  bytes(): Uint8Array {
    return requestHandles.of(this).bytes();
  }
}

let adoptableResponse: NativeHookResponse | null = null;

export class HookResponse {
  readonly outcome: HookOutcome;
  readonly value: HookJson | null;

  private constructor() {
    const handle = adoptableResponse;
    adoptableResponse = null;
    if (handle === null) {
      throw new ValueError('HookResponse has no public constructor');
    }
    this.outcome = handle.outcome() as HookOutcome;
    this.value = optionalJson(handle.value());
    responseHandles.attach(this, handle);
    Object.freeze(this);
  }

  static continuing(): HookResponse {
    return adoptResponse(callNative(() => native.hookResponseContinuing()));
  }

  static block(reason: string): HookResponse {
    return adoptResponse(callNative(() => native.hookResponseText('block', toUnits(reason))));
  }

  static ask(reason: string): HookResponse {
    return adoptResponse(callNative(() => native.hookResponseText('ask', toUnits(reason))));
  }

  static context(text: string): HookResponse {
    return adoptResponse(callNative(() => native.hookResponseText('context', toUnits(text))));
  }

  static modifyInput(event: HookEvent, input: HookJson): HookResponse {
    const handle = jsonHandle(input);
    return adoptResponse(callNative(() => native.hookResponseModifyInput(event, handle)));
  }

  static parse(data: Uint8Array, event: HookEvent, declared: readonly HookOutcome[]): HookResponse {
    return adoptResponse(
      callNative(() => native.hookResponseParse(requireData(data), event, [...declared])),
    );
  }

  bytes(): Uint8Array {
    return responseHandles.of(this).bytes();
  }
}

function adoptResponse(handle: NativeHookResponse): HookResponse {
  adoptableResponse = handle;
  try {
    return Reflect.construct(HookResponse, []) as HookResponse;
  } finally {
    adoptableResponse = null;
  }
}

let adoptableEffects: NativeHookEffects | null = null;

export class HookEffects {
  readonly input: HookJson | null;
  readonly contexts: readonly string[];
  readonly decision: HookOutcome | null;
  readonly reason: string | null;

  private constructor() {
    const handle = adoptableEffects;
    adoptableEffects = null;
    if (handle === null) {
      throw new ValueError('HookEffects has no public constructor');
    }
    const reason = handle.reason();
    const decision = handle.decision();
    this.input = optionalJson(handle.input());
    this.contexts = Object.freeze(handle.contexts().map(fromUnits));
    this.decision = decision === null ? null : (decision as HookOutcome);
    this.reason = reason === null ? null : fromUnits(reason);
    effectsHandles.attach(this, handle);
    Object.freeze(this);
  }

  static parse(data: Uint8Array): HookEffects {
    return adoptEffects(callNative(() => native.hookEffectsParse(requireData(data))));
  }

  static none(): HookEffects {
    return adoptEffects(callNative(() => native.hookEffectsNone()));
  }

  bytes(): Uint8Array {
    return effectsHandles.of(this).bytes();
  }
}

function adoptEffects(handle: NativeHookEffects): HookEffects {
  adoptableEffects = handle;
  try {
    return Reflect.construct(HookEffects, []) as HookEffects;
  } finally {
    adoptableEffects = null;
  }
}

export interface HookHandlerInit {
  readonly id: string;
  readonly event: HookEvent;
  readonly argv: readonly string[];
  readonly environment?: Readonly<Record<string, EnvironmentValue>>;
  readonly cwd: string;
  readonly timeoutMs: number;
  readonly outcomes: readonly HookOutcome[];
  readonly tools?: readonly string[] | null;
}

let adoptableHandler: NativeHookHandler | null = null;

export class HookHandler {
  readonly id: string;
  readonly event: HookEvent;
  readonly argv: readonly string[];
  readonly environment: Readonly<Record<string, EnvironmentValue>>;
  readonly cwd: string;
  readonly timeoutMs: number;
  readonly outcomes: readonly HookOutcome[];
  readonly tools: readonly string[];

  constructor(init: HookHandlerInit) {
    const adopted = adoptableHandler;
    adoptableHandler = null;
    const handle: NativeHookHandler =
      adopted ??
      callNative(() =>
        native.hookHandlerNew({
          id: requireText(init.id, 'hook handler id'),
          event: init.event,
          argv: init.argv.map((argument) => requireText(argument, 'hook command argument')),
          environment: Object.entries(init.environment ?? {}).map(([name, value]) => {
            if (typeof value !== 'string' && !(value instanceof EnvRef)) {
              throw new ValueError('hook environment values must be strings or EnvRef');
            }
            return {
              name: requireText(name, 'hook environment name'),
              reference: typeof value !== 'string',
              value: requireText(
                typeof value === 'string' ? value : value.name,
                'hook environment',
              ),
            };
          }),
          cwd: requireText(init.cwd, 'hook handler cwd'),
          timeoutMs: init.timeoutMs,
          outcomes: [...init.outcomes],
          tools: (init.tools ?? []).map((tool) => requireText(tool, 'hook tool name')),
        }),
      );
    const snapshot = handle.snapshot();
    this.id = snapshot.id;
    this.event = snapshot.event as HookEvent;
    this.argv = Object.freeze([...snapshot.argv]);
    this.environment = Object.freeze(
      Object.fromEntries(
        snapshot.environment.map(
          ({ name, reference, value }) =>
            [name, reference ? new EnvRef(value) : value] as [string, EnvironmentValue],
        ),
      ),
    );
    this.cwd = snapshot.cwd;
    this.timeoutMs = snapshot.timeoutMs;
    this.outcomes = Object.freeze(snapshot.outcomes as HookOutcome[]);
    this.tools = Object.freeze([...snapshot.tools]);
    handlerHandles.attach(this, handle);
    Object.freeze(this);
  }
}

function adoptHandler(handle: NativeHookHandler): HookHandler {
  adoptableHandler = handle;
  try {
    return new HookHandler(undefined as never);
  } finally {
    adoptableHandler = null;
  }
}

export class HookConfig {
  readonly host: string;
  readonly handlers: readonly HookHandler[];

  constructor(host: string, handlers: readonly HookHandler[]) {
    const handles = handlers.map((handler) => handlerHandles.of(handler));
    const handle: NativeHookConfig = callNative(() =>
      native.hookConfigNew(requireText(host, 'hook host'), handles),
    );
    this.host = handle.host();
    this.handlers = Object.freeze(handle.handlers().map(adoptHandler));
    configHandles.attach(this, handle);
    Object.freeze(this);
  }

  bytes(): Uint8Array {
    return configHandles.of(this).bytes();
  }
}
