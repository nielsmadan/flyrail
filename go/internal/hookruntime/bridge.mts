import { object, parse, readInput, report, run, RuntimeFailure } from './runner.mts';
import type { Config, Effects, EventName, Input, Json, NativeRecord, RecordValue } from './runner.mts';

function selected(raw: NativeRecord, names: Record<string, string>): NativeRecord {
  const context: NativeRecord = {};
  for (const [source, destination] of Object.entries(names)) if (raw[source] !== undefined && raw[source] !== null) context[destination] = raw[source];
  return context;
}
export function normalize(host: string, event: EventName, raw: Json): Input {
  if (!object(raw)) throw new RuntimeFailure('host-input');
  const copilot = host === 'copilot';
  const context = selected(raw, copilot ? { cwd: 'cwd', sessionId: 'session_id', timestamp: 'timestamp', stop_hook_active: 'stop_hook_active', stopReason: 'stop_reason' } : { cwd: 'cwd', session_id: 'session_id', transcript_path: 'transcript_path', permission_mode: 'permission_mode', model: 'model', stop_hook_active: 'stop_hook_active', conversation_id: 'conversation_id', generation_id: 'generation_id' });
  const input: Input = { version: 1, event, context };
  if (event === 'before-tool' || event === 'after-tool') {
    const tool = raw[copilot ? 'toolName' : 'tool_name'];
    if (typeof tool !== 'string' || !tool) throw new RuntimeFailure('host-tool');
    input.tool = tool;
    input.input = raw[copilot ? 'toolArgs' : 'tool_input'];
    if (input.input === undefined) throw new RuntimeFailure('host-arguments');
    const id = raw[copilot ? 'toolCallId' : 'tool_use_id'];
    if (typeof id === 'string') input.context.tool_call_id = id;
    if (event === 'after-tool') {
      if (host === 'cursor') {
        if (typeof raw.tool_output !== 'string') throw new RuntimeFailure('host-result');
        input.result = parse(Buffer.from(raw.tool_output));
      } else input.result = raw[copilot ? 'toolResult' : 'tool_response'];
      if (input.result === undefined) throw new RuntimeFailure('host-result');
    }
  } else if (event === 'prompt') {
    if (typeof raw.prompt !== 'string') throw new RuntimeFailure('host-prompt');
    input.input = raw.prompt;
  }
  return input;
}
export function nativeOutput(host: string, event: EventName, effects: Effects): RecordValue {
  const output: RecordValue = {};
  const context = effects.contexts.join('\n');
  if (host === 'claude' || host === 'codex') {
    const hookEventName = ({ 'session-start': 'SessionStart', 'before-tool': 'PreToolUse', 'after-tool': 'PostToolUse', prompt: 'UserPromptSubmit', stop: 'Stop' })[event];
    const specific: RecordValue = { hookEventName };
    if (context) specific.additionalContext = context;
    if (event === 'before-tool') {
      if (effects.decision) {
        specific.permissionDecision = effects.decision === 'block' ? 'deny' : 'ask';
        specific.permissionDecisionReason = effects.reason ?? '';
      }
      if (effects.input !== undefined && effects.decision !== 'block') specific.updatedInput = effects.input;
    } else if (effects.decision === 'block') { output.decision = 'block'; output.reason = effects.reason ?? ''; }
    if (Object.keys(specific).length > 1) output.hookSpecificOutput = specific;
  } else if (host === 'cursor') {
    if (context) output.additional_context = context;
    if (event === 'before-tool') {
      if (effects.decision) { output.permission = 'deny'; output.user_message = effects.reason ?? ''; }
    } else if (effects.decision) { output.continue = false; output.user_message = effects.reason ?? ''; }
  } else if (host === 'copilot') {
    if (context) output.additionalContext = context;
    if (effects.input !== undefined && effects.decision !== 'block') output.modifiedArgs = effects.input;
    if (effects.decision) { output.permissionDecision = effects.decision === 'block' ? 'deny' : 'ask'; output.permissionDecisionReason = effects.reason ?? ''; }
  }
  return output;
}
export async function native(config: Config, event: EventName): Promise<void> {
  try {
    const raw = await readInput(process.stdin);
    if (object(raw) && event === 'stop' && ((config.host === 'copilot' && raw.stopReason !== 'end_turn') || (config.host === 'cursor' && raw.status !== 'completed'))) {
      process.stdout.write('{}\n'); return;
    }
    const input = normalize(config.host, event, raw);
    const effects = await run(config, input);
    process.stdout.write(`${JSON.stringify(nativeOutput(config.host, event, effects))}\n`);
  } catch (error) { report(error); process.stdout.write('{}\n'); }
}
export type PiContext = { cwd: string; mode: string; hasUI: boolean };
export type PiEvent = NativeRecord;
export type Pi = { on(event: string, handler: (event: PiEvent, context: PiContext) => Promise<RecordValue | void>): void };
export function registerPi(config: Config, pi: Pi): void {
  const mapping: [EventName, string][] = [['session-start', 'session_start'], ['before-tool', 'tool_call'], ['after-tool', 'tool_result'], ['prompt', 'input'], ['stop', 'agent_settled']];
  for (const [event, nativeEvent] of mapping) {
    if (!config.handlers.some(handler => handler.event === event)) continue;
    pi.on(nativeEvent, async (raw, ctx): Promise<RecordValue | void> => {
      try {
        if (event === 'after-tool' && raw.isError === true) return;
        const input: Input = { version: 1, event, context: { cwd: ctx.cwd, execution_mode: ctx.mode, has_ui: ctx.hasUI } };
        if (event === 'before-tool' || event === 'after-tool') {
          if (typeof raw.toolName !== 'string' || !object(raw.input)) throw new RuntimeFailure('pi-tool');
          input.tool = raw.toolName; input.input = raw.input;
          if (typeof raw.toolCallId === 'string') input.context.tool_call_id = raw.toolCallId;
          if (event === 'after-tool') input.result = selected(raw, { content: 'content', details: 'details', isError: 'is_error', usage: 'usage' });
        }
        if (event === 'prompt') {
          if (typeof raw.text !== 'string') throw new RuntimeFailure('pi-prompt');
          input.input = raw.text;
          if (typeof raw.source === 'string') input.context.source = raw.source;
        }
        const effects = await run(config, input);
        if (effects.decision) return { block: true, reason: effects.reason ?? '' };
        if (event === 'before-tool' && object(effects.input) && object(raw.input)) {
          for (const key of Object.keys(raw.input)) delete raw.input[key];
          Object.assign(raw.input, effects.input);
        }
        if (event === 'prompt' && typeof effects.input === 'string') return { action: 'transform', text: effects.input };
        if (event === 'after-tool' && effects.contexts.length) {
          if (!Array.isArray(raw.content)) throw new RuntimeFailure('pi-content');
          return { content: [...raw.content, { type: 'text', text: effects.contexts.join('\n') }] };
        }
      } catch (error) { report(error); }
    });
  }
}
export type OpenCodeInput = { directory: string };
export type ToolBefore = { tool: string; sessionID: string; callID: string };
export type ToolAfter = ToolBefore & { args: unknown };
export type OpenCodeHooks = {
  'tool.execute.before'?: (input: ToolBefore, output: { args: unknown }) => Promise<void>;
  'tool.execute.after'?: (input: ToolAfter, output: { title: string; output: string; metadata?: unknown }) => Promise<void>;
  event?: (input: { event: { type: string; properties: unknown } }) => Promise<void>;
};
export function openCode(config: Config, host: OpenCodeInput): OpenCodeHooks {
  const context = (input: ToolBefore): RecordValue => ({ cwd: host.directory, session_id: input.sessionID, tool_call_id: input.callID });
  const hooks: OpenCodeHooks = {};
  if (config.handlers.some(handler => handler.event === 'before-tool')) hooks['tool.execute.before'] = async (input, output) => {
    let effects: Effects;
    try { effects = await run(config, { version: 1, event: 'before-tool', context: context(input), tool: input.tool, input: output.args }); }
    catch (error) { report(error); return; }
    if (effects.decision === 'block') throw new Error(effects.reason);
    if (effects.input !== undefined) output.args = effects.input;
  };
  if (config.handlers.some(handler => handler.event === 'after-tool')) hooks['tool.execute.after'] = async (input, output) => {
    try {
      const result: NativeRecord = { title: output.title, output: output.output };
      if (output.metadata !== undefined) result.metadata = output.metadata;
      const effects = await run(config, { version: 1, event: 'after-tool', context: context(input), tool: input.tool, input: input.args, result });
      if (effects.contexts.length) output.output += `\n${effects.contexts.join('\n')}`;
    } catch (error) { report(error); }
  };
  if (config.handlers.some(handler => handler.event === 'session-start')) hooks.event = async ({ event }) => {
    if (event.type !== 'session.created') return;
    try {
      const ctx: RecordValue = { cwd: host.directory };
      if (object(event.properties) && object(event.properties.info) && typeof event.properties.info.id === 'string') ctx.session_id = event.properties.info.id;
      await run(config, { version: 1, event: 'session-start', context: ctx });
    } catch (error) { report(error); }
  };
  return hooks;
}
