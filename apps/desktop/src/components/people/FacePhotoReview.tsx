import { useId, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { assignFacesToPerson, clearFaceDecision, createPerson, decideFace, setFaceClarity, getFaceReviewPage } from "@/lib/api";
import type { FaceCluster, FaceReviewItem, Person } from "@/types";
import type { MessageKey } from "@/lib/i18n";
import { FacePhotoPreview } from "./FacePhotoPreview";
import { actionTargets, faceIsBlurry, groupFacePhotos, reviewStatus, selectFaceRange, type FaceView, type ReviewStatus } from "./faceWorkbenchModel";

type Operation = { ids: string[]; personId?: string; name?: string; status?: "pending" | "notFace"; blurry?: boolean | null };

function PhotoSelection({ ids, selected, disabled, label, onChange }: {
  ids: string[]; selected: string[]; disabled: boolean; label: string;
  onChange: (checked: boolean, range: boolean) => void;
}) {
  const count = ids.filter((id) => selected.includes(id)).length;
  return <label className="face-photo__select">
    <input type="checkbox" aria-label={label} checked={count === ids.length} disabled={disabled}
      ref={(input) => { if (input) input.indeterminate = count > 0 && count < ids.length; }}
      onChange={(event) => onChange(event.target.checked, (event.nativeEvent as MouseEvent).shiftKey)} />
  </label>;
}

export function FacePhotoReview({ persons, clusters, t, onReveal, invalidate }: {
  persons: Person[]; clusters: FaceCluster[]; t: (key: MessageKey) => string;
  onReveal: (id: string) => void; invalidate: () => void;
}) {
  const client = useQueryClient();
  const groupIdPrefix = useId();
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const [view, setView] = useState<FaceView>("photos");
  const [status, setStatus] = useState<ReviewStatus | "all">("all");
  const [minScore, setMinScore] = useState(0);
  const [minPixels, setMinPixels] = useState(0);
  const [blurThreshold, setBlurThreshold] = useState(0.2);
  const [blurOnly, setBlurOnly] = useState(false);
  const [width, setWidth] = useState(320);
  const [selected, setSelected] = useState<string[]>([]);
  const anchor = useRef<string | undefined>(undefined);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState("");
  const [batchSnapshot, setBatchSnapshot] = useState<FaceReviewItem[]>();
  const resetSelection = () => { setSelected([]); anchor.current = undefined; };
  const review = useInfiniteQuery({
    queryKey: ["face-review", "photos"], initialPageParam: 0,
    queryFn: ({ pageParam }) => getFaceReviewPage(pageParam, 200),
    getNextPageParam: (page) => page.nextCursor ?? undefined,
  });
  const loadedItems = useMemo(() => {
    const unique = new Map<string, FaceReviewItem>();
    for (const page of review.data?.pages ?? []) for (const item of page.items) unique.set(item.observationId, item);
    return [...unique.values()];
  }, [review.data]);
  const items = batchSnapshot ?? loadedItems;
  const filtered = useMemo(() => items.filter((face) => (status === "all" || reviewStatus(face) === status)
    && face.detectionScore >= minScore && (face.facePixels ?? 0) >= minPixels
    && (!blurOnly || faceIsBlurry(face, blurThreshold) === true)), [items, status, minScore, minPixels, blurOnly, blurThreshold]);
  const groups = useMemo(() => groupFacePhotos(filtered, view, clusters), [filtered, view, clusters]);
  const expandedGroups = groups.filter((group) => view === "photos" || !collapsedGroups.has(`${view}:${group.id}`));
  const orderedIds = expandedGroups.flatMap((group) => group.cards.flatMap((card) => card.faces.map((face) => face.observationId)));
  const visibleIds = new Set(orderedIds);
  const selection = selected.filter((id) => visibleIds.has(id));
  const select = (id: string, toggle: boolean, range: boolean) => {
    setSelected(selectFaceRange(orderedIds, selection, id, anchor.current, toggle, range));
    if (!range) anchor.current = id;
  };
  const mutation = useMutation({
    mutationFn: async (operation: Operation) => {
      if (operation.blurry !== undefined) {
        await setFaceClarity(operation.ids, operation.blurry);
        return;
      }
      let personId = operation.personId;
      if (operation.name) personId = (await createPerson(crypto.randomUUID(), operation.name)).personId;
      if (personId) await assignFacesToPerson(personId, operation.ids);
      else for (const id of operation.ids) {
        if (operation.status === "pending") await clearFaceDecision(id);
        else await decideFace(id, { decision: "notFace" });
      }
    },
    onSuccess: (_result, operation) => {
      setNotice(t("facePhotosApplied").replace("{count}", String(operation.ids.length)));
      setDrafts({}); resetSelection();
    },
    onError: (error) => setNotice(`${t("facePhotosFailed")} ${String(error)}`),
    // Refresh even if a per-face operation partially succeeded.
    onSettled: async () => {
      invalidate();
      await client.invalidateQueries({ queryKey: ["face-review"] });
      setBatchSnapshot(undefined);
    },
  });
  const run = (face: FaceReviewItem, operation: Omit<Operation, "ids">) => {
    const ids = actionTargets(face.observationId, selection);
    if (!selection.includes(face.observationId)) setSelected(ids);
    setBatchSnapshot(items);
    mutation.mutate({ ...operation, ids });
  };
  const label = (face: FaceReviewItem) => reviewStatus(face) === "confirmed"
    ? `${t("peopleState_confirmed")} · ${face.confirmedPersonName ?? ""}`
    : reviewStatus(face) === "notFace" ? t("peopleNotFace") : t("peopleState_pending");

  return <section className="face-photos" aria-label={t("facePhotosTitle")}>
    <fieldset disabled={mutation.isPending} className="face-photos__toolbar">
      <div className="face-photos__views" role="group" aria-label={t("facePhotosView")}>
        {(["photos", "status", "similar"] as const).map((mode) => <button key={mode} type="button" disabled={mutation.isPending}
          aria-pressed={view === mode} onClick={() => { setView(mode); resetSelection(); }}>{t(`facePhotosView_${mode}`)}</button>)}
      </div>
      <label>{t("facePhotosStatus")}<select value={status} disabled={mutation.isPending} onChange={(event) => { setStatus(event.target.value as typeof status); resetSelection(); }}>
        <option value="all">{t("peopleFilter_all")}</option><option value="confirmed">{t("peopleState_confirmed")}</option>
        <option value="pending">{t("peopleState_pending")}</option><option value="notFace">{t("peopleNotFace")}</option>
      </select></label>
      <label>{t("peopleQualityDetection")}<input type="number" min={0} max={1} step={0.05} value={minScore} onChange={(event) => { setMinScore(Number(event.target.value)); resetSelection(); }} /></label>
      <label>{t("peopleQualityFacePixels")}<input type="number" min={0} step={8} value={minPixels} onChange={(event) => { setMinPixels(Number(event.target.value)); resetSelection(); }} /></label>
      <label>{t("facePhotosBlurThreshold")}<input type="number" min={0} max={1} step={0.05} value={blurThreshold} onChange={(event) => { setBlurThreshold(Number(event.target.value)); resetSelection(); }} /></label>
      <label><input type="checkbox" checked={blurOnly} onChange={(event) => { setBlurOnly(event.target.checked); resetSelection(); }} />{t("facePhotosBlurOnly")}</label>
      <label>{t("facePhotosSize")}<input type="range" min={280} max={480} step={20} value={width} onChange={(event) => setWidth(Number(event.target.value))} /></label>
    </fieldset>
    <div className="face-photos__selection">
      <span>{t("facePhotosSelected").replace("{count}", String(selection.length))}</span>
      <button type="button" disabled={mutation.isPending} onClick={() => setSelected(orderedIds)}>{t("facePhotosSelectLoaded")}</button>
      <button type="button" disabled={mutation.isPending || !selection.length} onClick={resetSelection}>{t("facePhotosClearSelection")}</button>
      <small>{t("facePhotosSelectionHint")}</small>
    </div>
    {notice ? <p role={mutation.isError ? "alert" : "status"}>{notice}</p> : null}
    {review.isError ? <p role="alert">{String(review.error)} <button type="button" onClick={() => void review.refetch()}>{t("facePhotosRetry")}</button></p> : null}
    <div className={`face-photos__groups${view !== "photos" ? " is-grouped" : ""}`} style={{ "--face-card-width": `${width}px` } as React.CSSProperties}>
      {groups.map((group, index) => {
        const collapseKey = `${view}:${group.id}`;
        const collapsed = view !== "photos" && collapsedGroups.has(collapseKey);
        const cardsId = `${groupIdPrefix}-${encodeURIComponent(collapseKey)}`;
        return <section className="face-photos__group" key={group.id}>
        {view !== "photos" ? <header><button className="face-photos__group-toggle" type="button"
          aria-expanded={!collapsed} aria-controls={cardsId} disabled={mutation.isPending}
          onClick={() => {
            setCollapsedGroups((current) => {
              const next = new Set(current);
              if (next.has(collapseKey)) next.delete(collapseKey);
              else next.add(collapseKey);
              return next;
            });
            // Hidden faces must not remain batch targets or reappear selected.
            if (!collapsed) {
              const ids = new Set(group.cards.flatMap((card) => card.faces.map((face) => face.observationId)));
              setSelected((current) => current.filter((id) => !ids.has(id)));
            }
            anchor.current = undefined;
          }}>
          {collapsed ? <ChevronRight size={16} aria-hidden="true" /> : <ChevronDown size={16} aria-hidden="true" />}
          <strong>{group.status === "notFace" ? t("peopleNotFace") : group.status === "confirmed"
            ? `${t("peopleState_confirmed")} · ${group.name ?? ""}` : view === "status" ? t("peopleState_pending")
            : t("facePhotosPendingCluster").replace("{group}", String(index + 1)).replace("{identity}", group.suggestedName
              ? t("facePhotosSuggestedIdentity").replace("{name}", group.suggestedName)
              : t("facePhotosUnknownIdentity").replace("{number}", String(index + 1)))}</strong><span>{group.cards.length}</span></button></header> : null}
        <div className="face-photos__cards" id={cardsId} hidden={collapsed}>
          {!collapsed && group.cards.map((card) => <article className="face-photo" key={card.path}>
            <PhotoSelection ids={card.faces.map((face) => face.observationId)} selected={selection} disabled={mutation.isPending}
              label={t("facePhotosSelectPhoto").replace("{name}", card.path.split(/[\\/]/).at(-1) ?? card.path).replace("{count}", String(card.faces.length))}
              onChange={(checked, range) => {
                const ids = card.faces.map((face) => face.observationId);
                if (range && anchor.current) {
                  setSelected([...new Set([...selectFaceRange(orderedIds, selection, ids.at(-1)!, anchor.current, false, true), ...ids])]);
                } else {
                  setSelected(checked ? [...new Set([...selection, ...ids])] : selection.filter((id) => !ids.includes(id)));
                  anchor.current = ids[0];
                }
              }} />
            <FacePhotoPreview path={card.path} faces={card.faces} selected={selection} onSelect={select} unavailable={t("facePhotosPreviewUnavailable")} />
            <div className="face-photo__filename"><span title={card.path}>{card.path.split(/[\\/]/).at(-1)}</span>
              <button type="button" onClick={() => onReveal(card.faces[0].observationId)}>{t("peopleViewOriginal")}</button></div>
            {card.faces.map((face, faceIndex) => <div className={`face-photo__face${selection.includes(face.observationId) ? " is-selected" : ""}`} key={face.observationId}>
              <div className="face-photo__identity">
                <span>#{faceIndex + 1} {label(face)}</span><span>{face.manualBlurry !== undefined ? t(face.manualBlurry ? "facePhotosManualBlurry" : "facePhotosManualClear") : faceIsBlurry(face, blurThreshold) === undefined ? t("facePhotosQualityUnknown") : faceIsBlurry(face, blurThreshold) ? t("facePhotosBlurry") : t("facePhotosClear")}</span>
              </div>
              {face.candidate && reviewStatus(face) === "pending" ? <small>{t("peopleCandidate").replace("{name}", face.candidate.personName).replace("{value}", face.candidate.similarity.toFixed(2))}</small> : null}
              <small>{t("facePhotosApplyCount").replace("{count}", String(actionTargets(face.observationId, selection).length))}</small>
              <fieldset disabled={mutation.isPending} className="face-photo__actions">
                <div className="face-photo__quality-actions" role="group" aria-label={t("facePhotosManualQuality")}>
                  <button type="button" aria-pressed={face.manualBlurry === true} onClick={() => run(face, { blurry: true })}>{t("facePhotosMarkBlurry")}</button>
                  <button type="button" aria-pressed={face.manualBlurry === false} onClick={() => run(face, { blurry: false })}>{t("facePhotosMarkClear")}</button>
                  <button type="button" onClick={() => run(face, { blurry: null })}>{t("facePhotosQualityAuto")}</button>
                </div>
                <select aria-label={t("peopleAssignExistingIdentity")} value="" onChange={(event) => { if (event.target.value) run(face, { personId: event.target.value }); }}>
                  <option value="">{t("peopleChooseIdentity")}</option>{persons.map((person) => <option key={person.personId} value={person.personId}>{person.displayName}</option>)}
                </select>
                {face.candidate ? <button type="button" onClick={() => run(face, { personId: face.candidate!.personId })}>{t("peopleConfirmMatch")}</button> : null}
                <button type="button" onClick={() => run(face, { status: "pending" })}>{t("peopleState_pending")}</button>
                <button type="button" onClick={() => run(face, { status: "notFace" })}>{t("peopleNotFace")}</button>
                <input aria-label={t("peopleNewIdentityPlaceholder")} placeholder={t("peopleNewIdentityPlaceholder")} value={drafts[face.observationId] ?? ""} onChange={(event) => setDrafts({ ...drafts, [face.observationId]: event.target.value })} />
                <button type="button" disabled={!drafts[face.observationId]?.trim()} onClick={() => run(face, { name: drafts[face.observationId].trim() })}>{t("peopleCreateAndAssignIdentity")}</button>
              </fieldset>
            </div>)}
          </article>)}
        </div>
      </section>;
      })}
    </div>
    {!groups.length ? <p className="people-panel__empty">{review.isPending ? t("peopleReviewLoading") : t("peopleReviewEmpty")}</p> : null}
    <div className="face-photos__pagination"><span>{t("facePhotosLoaded").replace("{loaded}", String(items.length)).replace("{total}", String(review.data?.pages[0]?.total ?? 0))}</span>
      {review.hasNextPage ? <button type="button" disabled={review.isFetchingNextPage || mutation.isPending} onClick={() => void review.fetchNextPage()}>{t("collectionLoadMore")}</button> : null}
    </div>
  </section>;
}
