import type { Agent, TargetScope } from './enums.js';
import { callNative } from './internal/call.js';
import { attachHandle, handleOf, hasHandle } from './internal/handles.js';
import { fromPairObjects, toPairObjects } from './internal/maps.js';
import { requireAbsolute } from './internal/paths.js';
import { requireText } from './internal/text.js';
import type { NativeTarget } from './native/binding.js';
import { native } from './native/loader.js';

export class Target {
  readonly root: string;
  readonly agent: Agent | null;
  readonly scope: TargetScope;
  readonly home: string | null;
  readonly environment: Readonly<Record<string, string>>;

  private constructor(handle: NativeTarget) {
    const snapshot = handle.snapshot();
    this.root = snapshot.root;
    this.agent = (snapshot.agent ?? null) as Agent | null;
    this.scope = snapshot.scope as TargetScope;
    this.home = snapshot.home ?? null;
    this.environment = fromPairObjects(snapshot.environment);
    attachHandle(this, handle);
    Object.freeze(this);
  }

  static directory(root: string): Target {
    return new Target(
      callNative(() => native.targetDirectory(requireAbsolute(root, 'target root'))),
    );
  }

  static project(agent: Agent, root: string): Target {
    return new Target(
      callNative(() => native.targetProject(agent, requireAbsolute(root, 'target root'))),
    );
  }

  static user(agent: Agent, home: string, environment?: Readonly<Record<string, string>>): Target {
    const pairs = toPairObjects(environment).map(({ key, value }) => ({
      key: requireText(key, 'environment name'),
      value: requireText(value, 'environment value'),
    }));
    return new Target(
      callNative(() =>
        native.targetUser(
          agent,
          requireAbsolute(home, 'home'),
          environment === undefined ? undefined : pairs,
        ),
      ),
    );
  }

  equals(other: Target): boolean {
    return (
      hasHandle(other, Target, native.NativeTarget) &&
      (handleOf(this) as NativeTarget).equals(handleOf(other) as NativeTarget)
    );
  }
}
