import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  clearFaceAnalysisData, deletePeopleAnnotations, getFaceSyncStatus, resolveFaceSyncConflict,
} from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { PortableFaceFact } from "@/types";

export function FaceSyncPanel({ t }: { t: (key: MessageKey) => string }) {
  const describe = (items: PortableFaceFact[] = []) => items
    .filter((item) => item.decision)
    .map((item) => {
      const state = item.decision?.decision === "notFace" ? "notFace"
        : item.decision?.decision === "rejectPerson" ? "rejected" : "confirmed";
      const position = `${Math.round(item.region.x * 100)}%, ${Math.round(item.region.y * 100)}%`;
      return `${item.personTagPath?.replaceAll("|", " / ") ?? item.personName ?? ""} ${t(`peopleState_${state}`)} (${position})`;
    }).join("; ") || t("faceSyncNoFacts");
  const client = useQueryClient();
  const status = useQuery({ queryKey: ["face-sync"], queryFn: getFaceSyncStatus, refetchInterval: 2000 });
  const resolve = useMutation({
    mutationFn: ({ path, remote }: { path: string; remote: boolean }) => resolveFaceSyncConflict(path, remote),
    onSuccess: () => {
      for (const key of ["face-sync", "face-review", "face-persons", "face-capability"]) {
        void client.invalidateQueries({ queryKey: [key] });
      }
    },
  });
  const clear = useMutation({
    mutationFn: (annotations: boolean) => annotations ? deletePeopleAnnotations() : clearFaceAnalysisData(),
    onSuccess: () => { void client.invalidateQueries(); },
  });

  return <section className="face-sync-panel" aria-label={t("faceSyncTitle")}>
    <h3>{t("faceSyncTitle")}</h3>
    <button
      type="button" disabled={clear.isPending}
      onClick={() => { if (window.confirm(t("faceClearCacheConfirm"))) clear.mutate(false); }}
    >{t("faceClearCache")}</button>
    <button
      type="button" disabled={clear.isPending}
      onClick={() => { if (window.confirm(t("faceDeleteFactsConfirm"))) clear.mutate(true); }}
    >{t("faceDeleteFacts")}</button>
    {clear.error ? <p role="alert">{String(clear.error)}</p> : null}
    {status.error || resolve.error ? <p role="alert">{String(status.error ?? resolve.error)}</p> : null}
    {status.data?.length ? (
      <p role="status">{t("faceSyncPending").replace("{count}", String(status.data.length))}</p>
    ) : null}
    {status.data?.slice(0, 20).map((item) => <div key={item.path}>
      <span title={item.path}>{item.path.split(/[\\/]/).pop()}</span>
      {item.conflict ? <>
        <p>{t("faceSyncConflict")}</p>
        <p>{t("faceSyncLocal")}: {describe(item.localFacts?.facts)}</p>
        <p>{t("faceSyncRemote")}: {describe(item.remoteFacts?.facts)}</p>
        <button type="button" disabled={resolve.isPending} onClick={() => resolve.mutate({ path: item.path, remote: false })}>
          {t("faceSyncKeepLocal")}
        </button>
        <button type="button" disabled={resolve.isPending} onClick={() => resolve.mutate({ path: item.path, remote: true })}>
          {t("faceSyncUseSidecar")}
        </button>
      </> : item.lastError ? <p>{t("faceSyncRetry")}</p> : null}
    </div>)}
  </section>;
}
