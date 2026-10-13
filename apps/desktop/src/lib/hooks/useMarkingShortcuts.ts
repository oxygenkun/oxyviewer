import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { patchMetadata } from "@/lib/api";
import { MARKING_ACTIONS, markingPatchForAction } from "@/lib/assets/assetMarking";
import { refreshTagQueries, TAG_MUTATION_SCOPE, toggleAssetCustomTag } from "@/lib/assets/tagActions";
import { applyMetadataProjectionPatch, useMetadataProjectionStore } from "@/lib/projection/metadataProjection";
import { matchesAction } from "@/lib/ui/shortcuts";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary, MetadataPatch } from "@/types";

function editableTarget(target: EventTarget | null): boolean {
  return target instanceof HTMLElement
    && Boolean(target.closest("input, textarea, select, [contenteditable='true'], [role='dialog'], [role='menu']"));
}

export function useMarkingShortcuts(
  assets: AssetSummary[],
  suppressed: boolean,
  fallbackActive?: AssetSummary,
) {
  const queryClient = useQueryClient();
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const activeId = useWorkspaceStore((state) => state.activeId);
  const activeAsset = assets.find((asset) => asset.id === activeId) ?? fallbackActive;
  const projection = useMetadataProjectionStore(
    (state) => activeAsset ? state.records[activeAsset.path] : undefined,
  );
  const patch = useMutation({
    mutationFn: ({ paths, value }: { paths: string[]; value: MetadataPatch }) =>
      patchMetadata(paths, value),
    onSuccess: (_data, { paths, value }) => applyMetadataProjectionPatch(paths, value),
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["asset-details"] }),
        queryClient.invalidateQueries({ queryKey: ["assets"] }),
        queryClient.invalidateQueries({ queryKey: ["preload-assets"] }),
        queryClient.invalidateQueries({ queryKey: ["progressive-metadata-assets"] }),
      ]);
    },
  });
  const repeatTag = useMutation({
    scope: TAG_MUTATION_SCOPE,
    mutationFn: ({ paths, tagId }: { paths: string[]; tagId: number }) => toggleAssetCustomTag(paths, tagId),
    // A mixed selection can have successful writes before another write fails.
    onSettled: () => refreshTagQueries(queryClient),
  });

  useEffect(() => {
    if (suppressed) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.repeat || event.isComposing || event.defaultPrevented) return;
      if (useWorkspaceStore.getState().settingsOpen || editableTarget(event.target)) return;
      const action = matchesAction(event, shortcuts, "marking.repeatLastTag")
        ? "marking.repeatLastTag"
        : MARKING_ACTIONS.find((candidate) => matchesAction(event, shortcuts, candidate));
      if (!action) return;
      const { selectedIds, lastTagId } = useWorkspaceStore.getState();
      const selected = assets.filter((asset) => selectedIds.includes(asset.id));
      const targets = selected.length ? selected : activeAsset ? [activeAsset] : [];
      if (action === "marking.repeatLastTag") {
        event.preventDefault();
        if (lastTagId !== undefined && targets.length) {
          repeatTag.mutate({ paths: targets.map((asset) => asset.path), tagId: lastTagId });
        }
        return;
      }
      if (!targets.length) return;
      const value = markingPatchForAction(action, projection?.colorLabel ?? activeAsset?.colorLabel);
      if (!value) return;
      event.preventDefault();
      patch.mutate({ paths: targets.map((asset) => asset.path), value });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeAsset, assets, fallbackActive, patch, projection, repeatTag, shortcuts, suppressed]);
}
