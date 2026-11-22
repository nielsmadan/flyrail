import type {
  Agent,
  Family,
  HookEvent,
  HookOutcome,
  Platform,
  Surface,
  TargetScope,
} from './enums.js';
import { contentFromWire, contentWire, treeFromWire, treeWire } from './content.js';
import type { Content, TreeContent } from './content.js';
import { ValueError } from './errors.js';
import { attachHandle } from './internal/handles.js';
import { fromPairs } from './internal/maps.js';
import {
  buildModel,
  type ArgumentWire,
  type ArtifactWire,
  type AssetRefWire,
  type AssetWire,
  type AudienceWire,
  type CommandWire,
  type EnvRefWire,
  type EnvironmentValueWire,
  type HookWire,
  type HttpWire,
  type InstructionWire,
  type McpWire,
  type NativeArtifactWire,
  type SkillWire,
} from './internal/models.js';
import { requireText } from './internal/text.js';

export class EnvRef {
  readonly name: string;

  constructor(name: string) {
    const { handle, snapshot } = buildModel<EnvRefWire>({
      kind: 'env-ref',
      name: requireText(name, 'environment reference'),
    });
    this.name = snapshot.name;
    attachHandle(this, handle);
    Object.freeze(this);
  }

  equals(other: EnvRef): boolean {
    return this.name === other.name;
  }
}

export class AssetRef {
  readonly assetId: string;
  readonly path: string | null;

  constructor(assetId: string, path: string | null = null) {
    const { handle, snapshot } = buildModel<AssetRefWire>({
      kind: 'asset-ref',
      assetId: requireText(assetId, 'asset id'),
      path: path === null ? null : requireText(path, 'asset reference path'),
    });
    this.assetId = snapshot.assetId;
    this.path = snapshot.path;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export type CommandArgument = string | AssetRef;
export type EnvironmentValue = string | EnvRef;

const argumentWire = (argument: CommandArgument): ArgumentWire =>
  typeof argument === 'string'
    ? { kind: 'text', text: requireText(argument, 'command argument') }
    : { kind: 'asset-ref', assetId: argument.assetId, path: argument.path };

const argumentFromWire = (wire: ArgumentWire): CommandArgument =>
  wire.kind === 'text' ? wire.text : new AssetRef(wire.assetId, wire.path);

const environmentWire = (value: EnvironmentValue): EnvironmentValueWire =>
  typeof value === 'string'
    ? { kind: 'text', text: requireText(value, 'environment value') }
    : { kind: 'env-ref', name: value.name };

const environmentFromWire = (wire: EnvironmentValueWire): EnvironmentValue =>
  wire.kind === 'text' ? wire.text : new EnvRef(wire.name);

export class Command {
  readonly kind = 'command';
  readonly argv: readonly CommandArgument[];
  readonly env: Readonly<Record<string, EnvironmentValue>>;
  readonly cwd: CommandArgument | null;

  constructor(
    argv: readonly CommandArgument[],
    env: Readonly<Record<string, EnvironmentValue>> = {},
    cwd: CommandArgument | null = null,
  ) {
    const { handle, snapshot } = buildModel<CommandWire>({
      kind: 'command',
      argv: argv.map(argumentWire),
      env: Object.entries(env).map(([name, value]) => ({
        name: requireText(name, 'environment name'),
        value: environmentWire(value),
      })),
      cwd: cwd === null ? null : argumentWire(cwd),
    });
    this.argv = Object.freeze(snapshot.argv.map(argumentFromWire));
    this.env = fromPairs(
      snapshot.env.map((entry) => [entry.name, environmentFromWire(entry.value)] as const),
    );
    this.cwd = snapshot.cwd === null ? null : argumentFromWire(snapshot.cwd);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

const commandWire = (command: Command): CommandWire => ({
  kind: 'command',
  argv: command.argv.map(argumentWire),
  env: Object.entries(command.env).map(([name, value]) => ({
    name,
    value: environmentWire(value),
  })),
  cwd: command.cwd === null ? null : argumentWire(command.cwd),
});

const commandFromWire = (wire: CommandWire): Command =>
  new Command(
    wire.argv.map(argumentFromWire),
    Object.fromEntries(wire.env.map((entry) => [entry.name, environmentFromWire(entry.value)])),
    wire.cwd === null ? null : argumentFromWire(wire.cwd),
  );

export class HttpTransport {
  readonly kind = 'http';
  readonly url: string;
  readonly bearerToken: EnvRef | null;

  constructor(url: string, bearerToken: EnvRef | null = null) {
    const { handle, snapshot } = buildModel<HttpWire>({
      kind: 'http',
      url: requireText(url, 'MCP URL'),
      bearerToken: bearerToken === null ? null : { kind: 'env-ref', name: bearerToken.name },
    });
    this.url = snapshot.url;
    this.bearerToken = bearerToken;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export type McpTransport = Command | HttpTransport;

export class Audience {
  readonly agent: Agent;
  readonly scope: TargetScope;
  readonly surface: Surface;
  readonly platform: Platform | null;

  constructor(
    agent: Agent,
    scope: TargetScope,
    surface: Surface,
    platform: Platform | null = null,
  ) {
    const { handle, snapshot } = buildModel<AudienceWire>({
      kind: 'audience',
      agent,
      scope,
      surface,
      platform,
    });
    this.agent = snapshot.agent;
    this.scope = snapshot.scope;
    this.surface = snapshot.surface;
    this.platform = snapshot.platform;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

const audienceWire = (audience: Audience): AudienceWire => ({
  kind: 'audience',
  agent: audience.agent,
  scope: audience.scope,
  surface: audience.surface,
  platform: audience.platform,
});

export class SkillArtifact {
  readonly kind = 'skill';
  readonly id: string;
  readonly name: string;
  readonly content: TreeContent;

  constructor(id: string, name: string, content: TreeContent) {
    const { handle, snapshot } = buildModel<SkillWire>({
      kind: 'skill',
      id: requireText(id, 'artifact id'),
      name: requireText(name, 'skill name'),
      content: treeWire(content),
    });
    this.id = snapshot.id;
    this.name = snapshot.name;
    this.content = treeFromWire(snapshot.content);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export class InstructionArtifact {
  readonly kind = 'instruction';
  readonly id: string;
  readonly text: string;

  constructor(id: string, text: string) {
    const { handle, snapshot } = buildModel<InstructionWire>({
      kind: 'instruction',
      id: requireText(id, 'artifact id'),
      text: requireText(text, 'instruction text'),
    });
    this.id = snapshot.id;
    this.text = snapshot.text;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

const transportWire = (transport: McpTransport): CommandWire | HttpWire => {
  switch (transport.kind) {
    case 'command':
      return commandWire(transport);
    case 'http':
      return {
        kind: 'http',
        url: transport.url,
        bearerToken:
          transport.bearerToken === null
            ? null
            : { kind: 'env-ref', name: transport.bearerToken.name },
      };
    default:
      throw new ValueError('MCP transport must be a Command or an HttpTransport');
  }
};

const transportFromWire = (wire: CommandWire | HttpWire): McpTransport =>
  wire.kind === 'command'
    ? commandFromWire(wire)
    : new HttpTransport(
        wire.url,
        wire.bearerToken === null ? null : new EnvRef(wire.bearerToken.name),
      );

export class McpArtifact {
  readonly kind = 'mcp';
  readonly id: string;
  readonly name: string;
  readonly transport: McpTransport;

  constructor(id: string, name: string, transport: McpTransport) {
    const { handle, snapshot } = buildModel<McpWire>({
      kind: 'mcp',
      id: requireText(id, 'artifact id'),
      name: requireText(name, 'MCP server name'),
      transport: transportWire(transport),
    });
    this.id = snapshot.id;
    this.name = snapshot.name;
    this.transport = transportFromWire(snapshot.transport);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export interface HookArtifactInit {
  readonly id: string;
  readonly event: HookEvent;
  readonly command: Command;
  readonly timeoutMs: number;
  readonly outcomes: readonly HookOutcome[];
  readonly tools?: readonly string[] | null;
}

export class HookArtifact {
  readonly kind = 'hook';
  readonly id: string;
  readonly event: HookEvent;
  readonly command: Command;
  readonly timeoutMs: number;
  readonly outcomes: readonly HookOutcome[];
  readonly tools: readonly string[];
  readonly protocolVersion: number;

  constructor(init: HookArtifactInit) {
    const { handle, snapshot } = buildModel<HookWire & { readonly protocolVersion: number }>({
      kind: 'hook',
      id: requireText(init.id, 'artifact id'),
      event: init.event,
      command: commandWire(init.command),
      timeoutMs: init.timeoutMs,
      outcomes: init.outcomes,
      tools: (init.tools ?? []).map((tool) => requireText(tool, 'tool name')),
    });
    this.id = snapshot.id;
    this.event = snapshot.event;
    this.command = commandFromWire(snapshot.command);
    this.timeoutMs = snapshot.timeoutMs;
    this.outcomes = Object.freeze([...snapshot.outcomes]);
    this.tools = Object.freeze([...snapshot.tools]);
    this.protocolVersion = snapshot.protocolVersion;
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export class NativeArtifact {
  readonly kind = 'native';
  readonly id: string;
  readonly family: Family;
  readonly audience: Audience;
  readonly destination: string;
  readonly content: Content;

  constructor(
    id: string,
    family: Family,
    audience: Audience,
    destination: string,
    content: Content,
  ) {
    const { handle, snapshot } = buildModel<NativeArtifactWire>({
      kind: 'native',
      id: requireText(id, 'artifact id'),
      family,
      audience: audienceWire(audience),
      destination: requireText(destination, 'native destination'),
      content: contentWire(content),
    });
    this.id = snapshot.id;
    this.family = snapshot.family;
    this.audience = audience;
    this.destination = snapshot.destination;
    this.content = contentFromWire(snapshot.content);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export type Artifact =
  SkillArtifact | InstructionArtifact | McpArtifact | HookArtifact | NativeArtifact;

export class SupportAsset {
  readonly id: string;
  readonly family: Family;
  readonly content: TreeContent;

  constructor(id: string, family: Family, content: TreeContent) {
    const { handle, snapshot } = buildModel<AssetWire>({
      kind: 'asset',
      id: requireText(id, 'asset id'),
      family,
      content: treeWire(content),
    });
    this.id = snapshot.id;
    this.family = snapshot.family;
    this.content = treeFromWire(snapshot.content);
    attachHandle(this, handle);
    Object.freeze(this);
  }
}

export function artifactWire(artifact: Artifact): ArtifactWire {
  switch (artifact.kind) {
    case 'skill':
      return {
        kind: 'skill',
        id: artifact.id,
        name: artifact.name,
        content: treeWire(artifact.content),
      };
    case 'instruction':
      return { kind: 'instruction', id: artifact.id, text: artifact.text };
    case 'mcp':
      return {
        kind: 'mcp',
        id: artifact.id,
        name: artifact.name,
        transport: transportWire(artifact.transport),
      };
    case 'hook':
      return {
        kind: 'hook',
        id: artifact.id,
        event: artifact.event,
        command: commandWire(artifact.command),
        timeoutMs: artifact.timeoutMs,
        outcomes: artifact.outcomes,
        tools: artifact.tools,
      };
    case 'native':
      return {
        kind: 'native',
        id: artifact.id,
        family: artifact.family,
        audience: audienceWire(artifact.audience),
        destination: artifact.destination,
        content: contentWire(artifact.content),
      };
  }
}

const audienceFromWire = (wire: AudienceWire): Audience =>
  new Audience(wire.agent, wire.scope, wire.surface, wire.platform);

export function artifactFromWire(wire: ArtifactWire): Artifact {
  switch (wire.kind) {
    case 'skill':
      return new SkillArtifact(wire.id, wire.name, treeFromWire(wire.content));
    case 'instruction':
      return new InstructionArtifact(wire.id, wire.text);
    case 'mcp':
      return new McpArtifact(wire.id, wire.name, transportFromWire(wire.transport));
    case 'hook':
      return new HookArtifact({
        id: wire.id,
        event: wire.event,
        command: commandFromWire(wire.command),
        timeoutMs: wire.timeoutMs,
        outcomes: wire.outcomes,
        tools: wire.tools,
      });
    case 'native':
      return new NativeArtifact(
        wire.id,
        wire.family,
        audienceFromWire(wire.audience),
        wire.destination,
        contentFromWire(wire.content),
      );
  }
}

export const assetWire = (asset: SupportAsset): AssetWire => ({
  kind: 'asset',
  id: asset.id,
  family: asset.family,
  content: treeWire(asset.content),
});

export const assetFromWire = (wire: AssetWire): SupportAsset =>
  new SupportAsset(wire.id, wire.family, treeFromWire(wire.content));
