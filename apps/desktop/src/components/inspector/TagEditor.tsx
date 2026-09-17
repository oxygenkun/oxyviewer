import { createPortal } from "react-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check, FolderInput, Minus, MoreHorizontal, Pencil, Plus, RotateCcw, Search, Trash2, X,
} from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  createCustomTag, deleteCustomTag, getAssetTagAssignments, getCustomTagDeleteImpact,
  getTagSyncStatus, listCustomTags, retryTagXmpSync, setAssetCustomTag, updateCustomTag,
} from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { descendantIds, embeddedOnlyTagPaths, ensureTagPath, parseTagPath } from "@/lib/assets/tagTree";
import type { CustomTag, TagDeleteImpact } from "@/types";

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
  const quickAddButtonRef = useRef<HTMLButtonElement>(null);
  const chipsRef = useRef<HTMLDivElement>(null);
  const pickerRef = useRef<HTMLDivElement>(null);
  const [pickerPosition, setPickerPosition] = useState({ top: 23, right: 15 });
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [creating, setCreating] = useState(false);
  const [newPath, setNewPath] = useState("");
  const [menuTagId, setMenuTagId] = useState<number>();
  const [form, setForm] = useState<TagForm>();
  const [removeTarget, setRemoveTarget] = useState<CustomTag>();
  const [deleteTarget, setDeleteTarget] = useState<CustomTag>();
  const tagsQuery = useQuery({ queryKey: ["custom-tags"], queryFn: listCustomTags });
  const assignmentsQuery = useQuery({
    queryKey: ["asset-tag-assignments", paths, sidecarKeywords, sidecarHierarchicalKeywords],
    queryFn: () => getAssetTagAssignments(paths),
    enabled: paths.length > 0,
  });
  const syncQuery = useQuery({
    queryKey: ["tag-sync-status"],
    queryFn: getTagSyncStatus,
    refetchInterval: (query) => query.state.data?.pendingCount ? 1_000 : false,
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
    const needle = search.trim().toLocaleLowerCase();
    return [...assignments]
      .filter(({ tag }) => !needle || tag.path.toLocaleLowerCase().includes(needle))
      .sort((left, right) => left.tag.path.localeCompare(right.tag.path));
  }, [assignments, search]);

  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["custom-tags"] }),
      queryClient.invalidateQueries({ queryKey: ["asset-tag-assignments"] }),
      queryClient.invalidateQueries({ queryKey: ["tag-sync-status"] }),
      queryClient.invalidateQueries({ queryKey: ["asset-details"] }),
      queryClient.invalidateQueries({ queryKey: ["assets"] }),
    ]);
  };
  const assignmentMutation = useMutation({
    mutationFn: ({ tagId, value }: { tagId: number; value: boolean }) =>
      setAssetCustomTag(paths, tagId, value),
    onSuccess: refresh,
  });
  const removeAssignmentMutation = useMutation({
    mutationFn: (tagId: number) => setAssetCustomTag([currentPath], tagId, false),
    onSuccess: async () => {
      setRemoveTarget(undefined);
      await refresh();
    },
  });
  const createPathMutation = useMutation({
    mutationFn: async (value: string) => {
      const availableTags = await listCustomTags();
      const tag = await ensureTagPath(value, availableTags, createCustomTag);
      await setAssetCustomTag(paths, tag.id, true);
      return tag;
    },
    onSuccess: async () => {
      setCreating(false);
      setNewPath("");
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
  const impactQuery = useQuery({
    queryKey: ["custom-tag-delete-impact", deleteTarget?.id],
    queryFn: () => getCustomTagDeleteImpact(deleteTarget!.id),
    enabled: Boolean(deleteTarget),
  });
  const deleteMutation = useMutation({
    mutationFn: (id: number) => deleteCustomTag(id),
    onSuccess: async () => {
      setDeleteTarget(undefined);
      setMenuTagId(undefined);
      await refresh();
    },
  });
  const retryMutation = useMutation({
    mutationFn: retryTagXmpSync,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tag-sync-status"] }),
  });
  const busy = assignmentMutation.isPending || removeAssignmentMutation.isPending || createPathMutation.isPending ||
    formMutation.isPending || deleteMutation.isPending;
  const startCreating = () => {
    setNewPath((current) => current || search.trim());
    setCreating(true);
    setMenuTagId(undefined);
  };
  const closeQuickAdd = () => {
    setOpen(false);
    setCreating(false);
    setForm(undefined);
    setMenuTagId(undefined);
    setRemoveTarget(undefined);
  };
  useEffect(() => {
    if (!open || deleteTarget || removeTarget) return;
    const dismissOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (pickerRef.current?.contains(target) || quickAddButtonRef.current?.contains(target)) return;
      setOpen(false);
      setCreating(false);
      setForm(undefined);
      setMenuTagId(undefined);
    };
    document.addEventListener("pointerdown", dismissOnOutsidePointer);
    return () => document.removeEventListener("pointerdown", dismissOnOutsidePointer);
  }, [deleteTarget, open, removeTarget]);
  useLayoutEffect(() => {
    if (!open && !deleteTarget && !removeTarget) return;
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
  }, [deleteTarget, open, removeTarget]);

  return (
    <div className="tag-editor">
      <div ref={chipsRef} className="tag-editor__chips">
        {assigned.map(({ tag, assignedCount, assetCount }) => (
          <span
            key={tag.id}
            className={`tag-chip${assignedCount === assetCount ? "" : " is-mixed"}`}
            title={displayTagPath(tag.path)}
          >
            <span>{displayTagPath(tag.path)}</span>
            <button
              className="tag-chip__delete"
              onClick={() => {
                setOpen(false);
                setRemoveTarget(tag);
              }}
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

      {syncQuery.data?.pendingCount ? (
        <div className={`tag-sync-state${syncQuery.data.failedCount ? " is-error" : ""}`}>
          <span>{syncQuery.data.failedCount ? t("tagSyncFailed") : t("tagSyncPending")}</span>
          {syncQuery.data.failedCount ? (
            <button onClick={() => retryMutation.mutate()} title={t("retry")}>
              <RotateCcw size={9} />
            </button>
          ) : null}
        </div>
      ) : null}

      {createPortal(open && !deleteTarget && !removeTarget ? (
        <div ref={pickerRef} className="tag-picker" style={pickerPosition} role="dialog" aria-label={t("quickAddTag")}>
          <div className="tag-picker__search">
            <Search />
            <input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("searchConfiguredTags")}
              autoFocus
            />
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
                    onClick={() => assignmentMutation.mutate({ tagId: tag.id, value: !complete })}
                    disabled={busy}
                    role="checkbox"
                    aria-checked={mixed ? "mixed" : complete}
                    aria-label={displayTagPath(tag.path)}
                  >{complete ? <Check /> : mixed ? <Minus /> : null}</button>
                  <button
                    className="tag-candidate__name"
                    onClick={() => assignmentMutation.mutate({ tagId: tag.id, value: !complete })}
                    disabled={busy}
                    title={displayTagPath(tag.path)}
                  >{displayTagPath(tag.path)}</button>
                  <button
                    className="tag-candidate__menu"
                    onClick={() => setMenuTagId((current) => current === tag.id ? undefined : tag.id)}
                    aria-label={t("manageTag")}
                  ><MoreHorizontal /></button>
                  {menuTagId === tag.id ? (
                    <div className="tag-row-menu" role="menu">
                      <button role="menuitem" onClick={() => setForm({ mode: "edit", tagId: tag.id, name: tag.name })}><Pencil />{t("renameTag")}</button>
                      <button role="menuitem" onClick={() => setForm({ mode: "move", tagId: tag.id, parentId: tag.parentId, name: tag.name })}><FolderInput />{t("moveTag")}</button>
                      <button role="menuitem" className="is-danger" onClick={() => setDeleteTarget(tag)}><Trash2 />{t("deleteTag")}</button>
                    </div>
                  ) : null}
                </div>
              );
            })}
            {candidates.length === 0 ? <em>{t("noMatchingTags")}</em> : null}
          </div>
          {creating ? (
            <InlineCreateTag
              value={newPath}
              t={t}
              pending={createPathMutation.isPending}
              error={createPathMutation.error}
              onChange={setNewPath}
              onCancel={() => setCreating(false)}
              onSave={() => createPathMutation.mutate(newPath)}
            />
          ) : (
            <button className="tag-picker__create" onClick={startCreating}>
              <Plus />{t("createTag")}
            </button>
          )}
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
      ) : removeTarget ? (
        <div
          className="tag-picker tag-picker--remove-confirmation"
          style={pickerPosition}
          role="dialog"
          aria-label={t("removeTagFromCurrent")}
        >
          <div className="tag-remove-confirmation">
            <span title={displayTagPath(removeTarget.path)}>
              {t("removeTagFromCurrent")}
            </span>
            <div>
              <button onClick={() => setRemoveTarget(undefined)} disabled={removeAssignmentMutation.isPending}>
                {t("cancel")}
              </button>
              <button
                className="is-danger"
                onClick={() => removeAssignmentMutation.mutate(removeTarget.id)}
                disabled={removeAssignmentMutation.isPending}
              >{t("removeTag")}</button>
            </div>
          </div>
        </div>
      ) : deleteTarget ? (
        <div className="tag-picker tag-picker--confirmation" style={pickerPosition} role="dialog" aria-label={t("tagDeleteTitle")}>
          <DeletePanel
            tag={deleteTarget}
            impact={impactQuery.data}
            t={t}
            pending={deleteMutation.isPending}
            onCancel={() => setDeleteTarget(undefined)}
            onDelete={() => deleteMutation.mutate(deleteTarget.id)}
          />
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

function InlineCreateTag({ value, t, pending, error, onChange, onCancel, onSave }: {
  value: string;
  t: (key: MessageKey) => string;
  pending: boolean;
  error: Error | null;
  onChange: (value: string) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const onCancelRef = useRef(onCancel);
  const composingRef = useRef(false);
  const segments = parseTagPath(value);
  useEffect(() => {
    onCancelRef.current = onCancel;
  }, [onCancel]);
  useEffect(() => {
    const dismissOnOutsidePointer = (event: PointerEvent) => {
      if (!panelRef.current?.contains(event.target as Node)) onCancelRef.current();
    };
    document.addEventListener("pointerdown", dismissOnOutsidePointer);
    return () => document.removeEventListener("pointerdown", dismissOnOutsidePointer);
  }, []);

  return (
    <div ref={panelRef} className="tag-picker__create-form">
      <Plus aria-hidden="true" />
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={t("tagPathExample")}
        title={t("tagPathHint")}
        autoFocus
        onCompositionStart={() => { composingRef.current = true; }}
        onCompositionEnd={() => { composingRef.current = false; }}
        onKeyDown={(event) => {
          if (event.key === "Escape") onCancel();
          const composing = composingRef.current || event.nativeEvent.isComposing || event.keyCode === 229;
          if (event.key === "Enter" && !composing && segments.length > 0) {
            event.preventDefault();
            onSave();
          }
        }}
      />
      <button onClick={onCancel} disabled={pending} aria-label={t("cancel")}><X /></button>
      <button className="is-primary" onClick={onSave} disabled={pending || segments.length === 0} aria-label={t("createAndAdd")}><Check /></button>
      {error ? <small className="metadata-error">{String(error)}</small> : null}
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

function DeletePanel({ tag, impact, t, pending, onCancel, onDelete }: {
  tag: CustomTag;
  impact?: TagDeleteImpact;
  t: (key: MessageKey) => string;
  pending: boolean;
  onCancel: () => void;
  onDelete: () => void;
}) {
  const body = t("tagDeleteBody")
    .replace("{tags}", String(impact?.tagCount ?? "…"))
    .replace("{assets}", String(impact?.assetCount ?? "…"));
  return (
    <div className="tag-edit-panel tag-delete-panel">
      <strong>{t("tagDeleteTitle")}</strong>
      <p><b>{displayTagPath(tag.path)}</b><br />{body}</p>
      <div>
        <button onClick={onCancel} disabled={pending}>{t("cancel")}</button>
        <button className="is-danger" onClick={onDelete} disabled={pending || !impact}>{t("deleteTag")}</button>
      </div>
    </div>
  );
}
