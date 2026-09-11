import type {
  AssetSummary,
  PreviewOmittedPolicy,
  PreviewScheduleIntent,
  RenderLevel,
  SchedulePlacement,
} from "../types";
import {
  reconcilePreviewSchedule,
  releasePreviewSchedule,
  upsertPreviewSchedule,
} from "./api";
import { orderBalancedAroundSelection, orderBySelectionPriority } from "./selectionPriority";

export interface PreviewViewportCandidate {
  asset: AssetSummary;
  visible: boolean;
  distance: number;
}

const RELEASE_OMITTED: PreviewOmittedPolicy = { action: "release" };

export const DEMOTE_OMITTED_TO_BACKGROUND: PreviewOmittedPolicy = {
  action: "demote",
  priority: "preload",
  placement: "back",
};

export interface PreviewScheduleScopeConfig {
  omittedPolicy?: PreviewOmittedPolicy;
  minDispatchIntervalMs?: number;
}

type PendingScheduleOperation =
  | {
      kind: "reconcile";
      intents: PreviewScheduleIntent[];
      omittedPolicy: PreviewOmittedPolicy;
      signature: string;
    }
  | {
      kind: "upsert";
      intent: Omit<PreviewScheduleIntent, "rank">;
      placement: SchedulePlacement;
      signature: string;
    }
  | {
      kind: "releaseTask";
      path: string;
      level: RenderLevel;
      signature: string;
    };

function reconcileSignature(
  intents: readonly PreviewScheduleIntent[],
  omittedPolicy: PreviewOmittedPolicy,
): string {
  const omitted = omittedPolicy.action === "release"
    ? "release"
    : `demote:${omittedPolicy.priority}:${omittedPolicy.placement}`;
  return `reconcile:${omitted}:${intents.map((intent) => (
    `${intent.path}\u0000${intent.level}\u0000${intent.priority}\u0000${intent.rank}`
  )).join("\u0001")}`;
}

export function viewportPreviewIntents(
  candidates: readonly PreviewViewportCandidate[],
  selected: AssetSummary | undefined,
  level: RenderLevel = "thumbnail",
): PreviewScheduleIntent[] {
  const selectedVisible = Boolean(
    selected && candidates.some(
      (candidate) => candidate.visible && candidate.asset.id === selected.id,
    ),
  );
  const visibleWithSelection = candidates.filter((candidate) => candidate.visible);
  const visible = visibleWithSelection.filter(
    (candidate) => !selectedVisible || candidate.asset.id !== selected?.id,
  );
  const orderedVisible = selectedVisible && selected
    ? orderBySelectionPriority(
        visibleWithSelection,
        selected.id,
        (candidate) => candidate.asset.id,
      ).filter((candidate) => candidate.asset.id !== selected.id)
    : [...visible].sort((left, right) => left.distance - right.distance);
  const nearby = candidates
    .filter((candidate) => !candidate.visible)
    .sort((left, right) => left.distance - right.distance);

  return [
    ...(selectedVisible && selected ? [{
      path: selected.path,
      level,
      priority: "loupe" as const,
      rank: 0,
    }] : []),
    ...orderedVisible.map(({ asset }, rank) => ({
      path: asset.path,
      level,
      priority: "visible" as const,
      rank,
    })),
    ...nearby.map(({ asset }, rank) => ({
      path: asset.path,
      level,
      priority: "nearby" as const,
      rank,
    })),
  ];
}

export function backgroundPreviewIntents(
  assets: readonly AssetSummary[],
  selectedId: string | undefined,
  level: RenderLevel = "thumbnail",
): PreviewScheduleIntent[] {
  const ordered = selectedId
    ? orderBalancedAroundSelection(assets, selectedId, (asset) => asset.id)
    : [...assets];
  return ordered.map((asset, rank) => ({
    path: asset.path,
    level,
    priority: "preload",
    rank,
  }));
}

/** Loupe still owns its selected base while the filmstrip scrolls elsewhere. */
export function filmstripPreviewIntents(
  candidates: readonly PreviewViewportCandidate[],
  selected: AssetSummary,
): PreviewScheduleIntent[] {
  return [
    { path: selected.path, level: "thumbnail", priority: "loupe", rank: 0 },
    ...viewportPreviewIntents(candidates, selected).filter((intent) => intent.path !== selected.path),
  ];
}

export class PreviewScheduleScope {
  readonly id: string;
  private epoch = 0;
  private readonly omittedPolicy: PreviewOmittedPolicy;
  private readonly minDispatchIntervalMs: number;
  private pending: PendingScheduleOperation[] = [];
  private lastAcceptedSignature: string | undefined;
  private lastDispatchAt = Number.NEGATIVE_INFINITY;
  private frameId: number | undefined;
  private timerId: ReturnType<typeof setTimeout> | undefined;
  private transportBusy = false;

  constructor(name: string, config: PreviewScheduleScopeConfig = {}) {
    this.id = `${name}:${crypto.randomUUID()}`;
    this.omittedPolicy = config.omittedPolicy ?? RELEASE_OMITTED;
    this.minDispatchIntervalMs = Math.max(0, config.minDispatchIntervalMs ?? 50);
  }

  reconcile(
    intents: PreviewScheduleIntent[],
    omittedPolicy: PreviewOmittedPolicy = this.omittedPolicy,
  ): void {
    const signature = reconcileSignature(intents, omittedPolicy);
    if (signature === this.lastAcceptedSignature) return;
    this.lastAcceptedSignature = signature;
    const operation: PendingScheduleOperation = {
      kind: "reconcile",
      intents: [...intents],
      omittedPolicy,
      signature,
    };
    if (this.pending.at(-1)?.kind === "reconcile") {
      this.pending[this.pending.length - 1] = operation;
    } else {
      this.pending.push(operation);
    }
    this.scheduleFlush();
  }

  upsert(
    intent: Omit<PreviewScheduleIntent, "rank">,
    placement: SchedulePlacement,
  ): void {
    this.enqueuePointOperation({
      kind: "upsert",
      intent: { ...intent },
      placement,
      signature: `upsert:${intent.path}\u0000${intent.level}\u0000${intent.priority}\u0000${placement}`,
    });
  }

  releaseTask(path: string, level: RenderLevel): void {
    this.enqueuePointOperation({
      kind: "releaseTask",
      path,
      level,
      signature: `releaseTask:${path}\u0000${level}`,
    });
  }

  release(): void {
    this.cancelScheduledFlush();
    this.pending = [{
      kind: "reconcile",
      intents: [],
      omittedPolicy: RELEASE_OMITTED,
      signature: reconcileSignature([], RELEASE_OMITTED),
    }];
    this.lastAcceptedSignature = this.pending[0].signature;
    this.dispatchNext();
  }

  private enqueuePointOperation(
    operation: Extract<PendingScheduleOperation, { kind: "upsert" | "releaseTask" }>,
  ): void {
    if (operation.signature === this.lastAcceptedSignature) return;
    this.lastAcceptedSignature = operation.signature;
    const previous = this.pending.at(-1);
    const previousPath = previous?.kind === "upsert"
      ? previous.intent.path
      : previous?.kind === "releaseTask"
        ? previous.path
        : undefined;
    const previousLevel = previous?.kind === "upsert"
      ? previous.intent.level
      : previous?.kind === "releaseTask"
        ? previous.level
        : undefined;
    const sameTask = previous?.kind !== "reconcile"
      && previousPath === (operation.kind === "upsert" ? operation.intent.path : operation.path)
      && previousLevel === (operation.kind === "upsert" ? operation.intent.level : operation.level);
    if (sameTask) this.pending[this.pending.length - 1] = operation;
    else this.pending.push(operation);
    this.scheduleFlush();
  }

  private scheduleFlush(): void {
    if (
      this.transportBusy
      || this.frameId !== undefined
      || this.timerId !== undefined
      || this.pending.length === 0
    ) return;
    const remaining = this.minDispatchIntervalMs - (performance.now() - this.lastDispatchAt);
    if (remaining > 0) {
      this.timerId = setTimeout(() => {
        this.timerId = undefined;
        this.dispatchNext();
      }, remaining);
      return;
    }
    if (typeof requestAnimationFrame === "function") {
      this.frameId = requestAnimationFrame(() => {
        this.frameId = undefined;
        this.dispatchNext();
      });
    } else {
      this.timerId = setTimeout(() => {
        this.timerId = undefined;
        this.dispatchNext();
      }, 0);
    }
  }

  private cancelScheduledFlush(): void {
    if (this.frameId !== undefined && typeof cancelAnimationFrame === "function") {
      cancelAnimationFrame(this.frameId);
    }
    if (this.timerId !== undefined) clearTimeout(this.timerId);
    this.frameId = undefined;
    this.timerId = undefined;
  }

  private dispatchNext(): void {
    if (this.transportBusy) return;
    const operation = this.pending.shift();
    if (!operation) return;
    this.lastDispatchAt = performance.now();
    this.epoch += 1;
    const epoch = this.epoch;
    this.transportBusy = true;
    const transport = operation.kind === "reconcile"
      ? reconcilePreviewSchedule(this.id, epoch, operation.intents, operation.omittedPolicy)
      : operation.kind === "upsert"
        ? upsertPreviewSchedule(this.id, epoch, operation.intent, operation.placement)
        : releasePreviewSchedule(this.id, epoch, operation.path, operation.level);
    void transport
      .then(() => undefined, () => undefined)
      .finally(() => {
        this.transportBusy = false;
        this.scheduleFlush();
      });
  }
}
