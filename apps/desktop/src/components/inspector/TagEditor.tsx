import { createPortal } from "react-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check, FolderInput, Minus, MoreHorizontal, Pencil, Plus, Search, Settings2, Trash2, X,
} from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  createCustomTag, deleteCustomTag, getAssetTagAssignments, getAssetTagSourceKinds,
  listCustomTags, setAssetCustomTag, updateCustomTag,
} from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { descendantIds, embeddedOnlyTagPaths, ensureTagPath, parseTagPath, sortTagCandidates, type TagPickerSort } from "@/lib/assets/tagTree";
import { refreshTagQueries, TAG_MUTATION_SCOPE } from "@/lib/assets/tagActions";
import { enqueueTagDeletion, hasPendingRemoval, pendingDeletedTagIds, useTagDeletions } from "@/lib/assets/tagDeletion";
import { useWorkspaceStore } from "@/store";
import type { CustomTag } from "@/types";

interface TagEditorProps {
  currentPath: string;
  paths: string[];
  sidecarKeywords: string[];
  sidecarHierarchicalKeywords: string[];
  embeddedKeywords: string[];
  embeddedHierarchicalKeywords: string[];
  t: (key: MessageKey) => string;
}

interface TagForm {
  mode: "edit" | "move";
  tagId: number;
  parentId?: number;
  name: string;
}

const SORT_STORAGE_KEY = "oxyviewer.tag-picker-sort.v1";
function readTagSort(): TagPickerSort {
  try {
    const value = localStorage.getItem(SORT_STORAGE_KEY);
    if (value === "recent" || value === "mostUsed" || value === "tree") return value;
  } catch { /* Preferences remain usable when storage is unavailable. */ }
  return "tree";
}

const displayTagPath = (path: string) => path.replaceAll("|", " › ");
export function TagEditor({
  currentPath,
  paths,
  sidecarKeywords,
  sidecarHierarchicalKeywords,
  embeddedKeywords,
  embeddedHierarchicalKeywords,
  t,
}: TagEditorProps) {
  const queryClient = useQueryClient();
  const setLastTagId = useWorkspaceStore((state) => state.setLastTagId);
  const quickAddButtonRef = useRef<HTMLButtonElement>(null);
  const chipsRef = useRef<HTMLDivElement>(null);
  const pickerRef = useRef<HTMLDivElement>(null);
  const [pickerPosition, setPickerPosition] = useState({ top: 23, right: 15 });
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<TagPickerSort>(readTagSort);
  const changeSort = (value: TagPickerSort) => {
    setSort(value);
    try { localStorage.setItem(SORT_STORAGE_KEY, value); } catch { /* Keep the session preference. */ }
  };
  const [menuTagId, setMenuTagId] = useState<number>();
  const [form, setForm] = useState<TagForm>();
  const [hint, setHint] = useState<string>();
  const pendingDeletions = useTagDeletions((state) => state.requests);
  const deletedTagIds = useMemo(() => pendingDeletedTagIds(pendingDeletions), [pendingDeletions]);
  const removedFromSelection = (tagId: number) =>
    paths.length > 0 && paths.every((path) => hasPendingRemoval(pendingDeletions, path, tagId));
  const tagsQuery = useQuery({ queryKey: ["custom-tags"], queryFn: listCustomTags });
  const assignmentsQuery = useQuery({
    queryKey: ["asset-tag-assignments", paths, sidecarKeywords, sidecarHierarchicalKeywords],
    queryFn: () => getAssetTagAssignments(paths),
    enabled: paths.length > 0,
  });
  const assignments = assignmentsQuery.data ?? [];
  const assigned = assignments.filter((item) => item.assignedCount > 0);
  const embeddedOnly = useMemo(
    () => embeddedOnlyTagPaths(
      assigned.map(({ tag }) => tag),
      embeddedKeywords,
      embeddedHierarchicalKeywords,
    ),
    [assigned, embeddedHierarchicalKeywords, embeddedKeywords],
  );
  const candidates = useMemo(() => {
    const needle = parseTagPath(search).join("|").toLocaleLowerCase();
    return sortTagCandidates(assignments, sort)
      .filter(({ tag }) => !deletedTagIds.has(tag.id) && (!needle || tag.path.toLocaleLowerCase().includes(needle)));
  }, [assignments, deletedTagIds, search, sort]);

  const searchPath = parseTagPath(search).join("|");
  const canCreate = tagsQuery.isSuccess && assignmentsQuery.isSuccess && searchPath.length > 0 &&
    !(tagsQuery.data ?? []).some((tag) =>
      tag.path.toLocaleLowerCase() === searchPath.toLocaleLowerCase() ||
      (!searchPath.includes("|") && tag.name.toLocaleLowerCase() === searchPath.toLocaleLowerCase()));

  const refresh = () => refreshTagQueries(queryClient);
  const assignmentMutation = useMutation({
    scope: TAG_MUTATION_SCOPE,
    mutationFn: ({ paths: targets, tagId, value }: { paths: string[]; tagId: number; value: boolean }) =>
      setAssetCustomTag(targets, tagId, value),
    onSuccess: async (_data, { tagId }) => {
      setLastTagId(tagId);
      await refresh();
    },
  });
  const removeAssignmentMutation = useMutation({
    scope: TAG_MUTATION_SCOPE,
    mutationFn: ({ path, tagId }: { path: string; tagId: number }) => setAssetCustomTag([path], tagId, false),
    onSuccess: async (_data, { tagId }) => {
      setLastTagId(tagId);
      await refresh();
    },
  });
  const createPathMutation = useMutation({
    scope: TAG_MUTATION_SCOPE,
    mutationFn: async ({ value, paths: targets }: { value: string; paths: string[] }) => {
      const availableTags = await listCustomTags();
      const tag = await ensureTagPath(value, availableTags, createCustomTag);
      await setAssetCustomTag(targets, tag.id, true);
      return tag;
    },
    onSuccess: async (tag) => {
      setLastTagId(tag.id);
      setSearch("");
      await refresh();
    },
  });
  const formMutation = useMutation({
    mutationFn: async (value: TagForm) => {
      const tag = assignments.find((item) => item.tag.id === value.tagId)?.tag;
      if (!tag) throw new Error("Tag no longer exists");
      return updateCustomTag(
        tag.id,
        value.mode === "move" ? value.parentId : tag.parentId,
        value.mode === "edit" ? value.name : tag.name,
      );
    },
    onSuccess: async () => {
      setForm(undefined);
      setMenuTagId(undefined);
      await refresh();
    },
  });
  const deleteMutation = useMutation({
    scope: TAG_MUTATION_SCOPE,
    mutationFn: (id: number) => deleteCustomTag(id),
  });
  const busy = assignmentMutation.isPending || createPathMutation.isPending || formMutation.isPending;
  const removeFromPhoto = async (tag: CustomTag) => {
    setOpen(false);
    setMenuTagId(undefined);
    let sources: string[];
    try {
      sources = await getAssetTagSourceKinds(currentPath, tag.id);
    } catch (error) {
      setHint(String(error));
      return;
    }
    // Person contributions belong to the person review workflow; removal cannot withdraw them.
    if (!sources.some((kind) => kind === "manual" || kind === "legacy" || kind === "sidecar")) {
      setHint(t("tagPersonSourceHint"));
      return;
    }
    setHint(undefined);
    enqueueTagDeletion(
      { kind: "assignment", tagPath: tag.path, tagIds: new Set([tag.id]), path: currentPath },
      () => removeAssignmentMutation.mutateAsync({ path: currentPath, tagId: tag.id }),
    );
  };
  const deleteTag = (tag: CustomTag) => {
    const removed = descendantIds(tagsQuery.data ?? [], tag.id);
    setMenuTagId(undefined);
    setHint(undefined);
    enqueueTagDeletion({ kind: "tag", tagPath: tag.path, tagIds: removed }, async () => {
      await deleteMutation.mutateAsync(tag.id);
      const lastTagId = useWorkspaceStore.getState().lastTagId;
      if (lastTagId !== undefined && removed.has(lastTagId)) setLastTagId(undefined);
      await refresh();
    });
  };
  const closeQuickAdd = () => {
    setOpen(false);
    setForm(undefined);
    setMenuTagId(undefined);
  };
  useEffect(() => {
    if (!hint) return;
    const timer = setTimeout(() => setHint(undefined), 8_000);
    return () => clearTimeout(timer);
  }, [hint]);
  useEffect(() => {
    if (!open) return;
    const dismissOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (pickerRef.current?.contains(target) || quickAddButtonRef.current?.contains(target)) return;
      setOpen(false);
      setForm(undefined);
      setMenuTagId(undefined);
    };
    document.addEventListener("pointerdown", dismissOnOutsidePointer);
    return () => document.removeEventListener("pointerdown", dismissOnOutsidePointer);
  }, [open]);
  useLayoutEffect(() => {
    if (!open) return;
    const updatePosition = () => {
      const button = quickAddButtonRef.current;
      if (button) {
        const rect = button.getBoundingClientRect();
        setPickerPosition({ top: rect.bottom + 3, right: Math.max(15, window.innerWidth - rect.right) });
      }
    };
    updatePosition();
    const chips = chipsRef.current;
    if (!chips) return;
    const observer = new ResizeObserver(updatePosition);
    observer.observe(chips);
    window.addEventListener("resize", updatePosition);
    document.addEventListener("scroll", updatePosition, true);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", updatePosition);
      document.removeEventListener("scroll", updatePosition, true);
    };
  }, [open]);

  return (
    <div className="tag-editor">
      <div ref={chipsRef} className="tag-editor__chips">
        {assigned.filter(({ tag }) => !deletedTagIds.has(tag.id) && !removedFromSelection(tag.id))
          .map(({ tag, assignedCount, assetCount }) => (
          <span
            key={tag.id}
            className={`tag-chip${assignedCount === assetCount ? "" : " is-mixed"}`}
            title={displayTagPath(tag.path)}
          >
            <span>{displayTagPath(tag.path)}</span>
            <button
              className="tag-chip__delete"
              onClick={() => void removeFromPhoto(tag)}
              aria-label={`${t("removeTagFromCurrent")} ${displayTagPath(tag.path)}`}
              disabled={busy}
            ><X /></button>
          </span>
        ))}
        {embeddedOnly.map((path) => (
          <span
            key={`embedded-${path}`}
            className="tag-chip is-embedded"
            title={`${t("embeddedTag")}: ${displayTagPath(path)}`}
          >
            <span>{displayTagPath(path)}</span>
          </span>
        ))}
        <button
          ref={quickAddButtonRef}
          className="tag-editor__quick-add"
          onClick={() => { if (open) closeQuickAdd(); else setOpen(true); }}
          aria-expanded={open}
        ><Plus />{t("quickAddTag")}</button>
      </div>

      {hint ? <small className="tag-editor__hint" role="status">{hint}</small> : null}

      {createPortal(open ? (
        <div ref={pickerRef} className="tag-picker" style={pickerPosition} role="dialog" aria-label={t("quickAddTag")}>
          <div className="tag-picker__search">
            <Search />
            <input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("searchConfiguredTags")}
              autoFocus
            />
            {canCreate ? (
              <button className="tag-picker__create" aria-label={t("createAndAdd")}
                title={`${t("createAndAdd")}: ${displayTagPath(searchPath)}`}
                disabled={busy}
                onClick={() => createPathMutation.mutate({ value: searchPath, paths })}
              >{t("createTag")}</button>
            ) : null}
            <label className="tag-picker__settings" title={t("tagSort")}>
              <Settings2 aria-hidden="true" />
              <select aria-label={t("tagSort")} value={sort}
                onChange={(event) => changeSort(event.target.value as TagPickerSort)}>
                <option value="recent">{t("tagSortRecent")}</option>
                <option value="mostUsed">{t("tagSortMostUsed")}</option>
                <option value="tree">{t("tagSortTree")}</option>
              </select>
            </label>
            <button onClick={closeQuickAdd} aria-label={t("cancel")}><X /></button>
          </div>
          <div className="tag-picker__list">
            {candidates.map(({ tag, assignedCount, assetCount }) => {
              const complete = assignedCount === assetCount && assetCount > 0;
              const mixed = assignedCount > 0 && !complete;
              return (
                <div className="tag-candidate" key={tag.id}>
                  <button
                    className={`tag-candidate__check${complete ? " is-checked" : mixed ? " is-mixed" : ""}`}
                    onClick={() => assignmentMutation.mutate({ paths, tagId: tag.id, value: !complete })}
                    disabled={busy}
                    role="checkbox"
                    aria-checked={mixed ? "mixed" : complete}
                    aria-label={displayTagPath(tag.path)}
                  >{complete ? <Check /> : mixed ? <Minus /> : null}</button>
                  <button
                    className="tag-candidate__name"
                    onClick={() => assignmentMutation.mutate({ paths, tagId: tag.id, value: !complete })}
                    disabled={busy}
                    title={displayTagPath(tag.path)}
                    style={sort === "tree" && !search.trim() ? { paddingLeft: 4 + (tag.path.split("|").length - 1) * 12 } : undefined}
                  >{sort === "tree" && !search.trim() ? tag.name : displayTagPath(tag.path)}</button>
                  <button
                    className="tag-candidate__menu"
                    onClick={() => setMenuTagId((current) => current === tag.id ? undefined : tag.id)}
                    aria-label={t("manageTag")}
                  ><MoreHorizontal /></button>
                  {menuTagId === tag.id ? (
                    <div className="tag-row-menu" role="menu">
                      <button role="menuitem" onClick={() => setForm({ mode: "edit", tagId: tag.id, name: tag.name })}><Pencil />{t("renameTag")}</button>
                      <button role="menuitem" onClick={() => setForm({ mode: "move", tagId: tag.id, parentId: tag.parentId, name: tag.name })}><FolderInput />{t("moveTag")}</button>
                      <button role="menuitem" className="is-danger" onClick={() => deleteTag(tag)}><Trash2 />{t("deleteTag")}</button>
                    </div>
                  ) : null}
                </div>
              );
            })}
            {candidates.length === 0 ? (
              <em>
                {t("noMatchingTags")}
                {canCreate ? <span className="tag-picker__hierarchy-hint">{t("tagHierarchyHint")}</span> : null}
              </em>
            ) : null}
          </div>
          {createPathMutation.error ? <small className="metadata-error" role="alert">{String(createPathMutation.error)}</small> : null}
          {form ? (
            <TagFormPanel
              form={form}
              tags={tagsQuery.data ?? []}
              t={t}
              pending={formMutation.isPending}
              error={formMutation.error}
              onChange={setForm}
              onCancel={() => setForm(undefined)}
              onSave={() => formMutation.mutate(form)}
            />
          ) : null}
        </div>
      ) : null, document.body)}
      {assignmentMutation.isError || removeAssignmentMutation.isError ? (
        <small className="metadata-error">
          {String(assignmentMutation.error ?? removeAssignmentMutation.error)}
        </small>
      ) : null}
    </div>
  );
}

function TagFormPanel({ form, tags, t, pending, error, onChange, onCancel, onSave }: {
  form: TagForm;
  tags: CustomTag[];
  t: (key: MessageKey) => string;
  pending: boolean;
  error: Error | null;
  onChange: (form: TagForm) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const descendants = descendantIds(tags, form.tagId);
  const composingRef = useRef(false);
  return (
    <div className="tag-edit-panel">
      <strong>{form.mode === "edit" ? t("renameTag") : t("moveTag")}</strong>
      {form.mode === "edit" ? (
        <input
          value={form.name}
          onChange={(event) => onChange({ ...form, name: event.target.value })}
          placeholder={t("tagName")}
          autoFocus
          onCompositionStart={() => { composingRef.current = true; }}
          onCompositionEnd={() => { composingRef.current = false; }}
          onKeyDown={(event) => {
            const composing = composingRef.current || event.nativeEvent.isComposing || event.keyCode === 229;
            if (event.key === "Enter" && !composing && form.name.trim()) {
              event.preventDefault();
              onSave();
            }
          }}
        />
      ) : (
        <select
          value={form.parentId ?? ""}
          onChange={(event) => onChange({ ...form, parentId: event.target.value ? Number(event.target.value) : undefined })}
          aria-label={t("moveTag")}
        >
          <option value="">{t("rootLevel")}</option>
          {tags.filter((tag) => !descendants.has(tag.id)).map((tag) => (
            <option key={tag.id} value={tag.id}>{displayTagPath(tag.path)}</option>
          ))}
        </select>
      )}
      {error ? <small className="metadata-error">{String(error)}</small> : null}
      <div>
        <button onClick={onCancel} disabled={pending}>{t("cancel")}</button>
        <button className="is-primary" onClick={onSave} disabled={pending || !form.name.trim()}>{t("save")}</button>
      </div>
    </div>
  );
}
