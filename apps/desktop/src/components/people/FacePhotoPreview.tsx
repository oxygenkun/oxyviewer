import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { faceWorkbenchPreview, renewMediaResource } from "@/lib/api";
import { retainMediaResource, releaseUnretainedMediaResource } from "@/lib/cache/mediaResourceLease";
import { previewContentStyles, validPreviewGeometry } from "@/lib/preview/previewGeometry";
import { RegionOverlay } from "@/components/analyzers/RegionOverlay";
import type { FaceReviewItem } from "@/types";

interface Props {
  path: string; faces: FaceReviewItem[]; selected: string[];
  onSelect: (id: string, toggle: boolean, range: boolean) => void;
  unavailable: string;
}

export function FacePhotoPreview(props: Props) {
  const host = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting), { rootMargin: "200px" });
    if (host.current) observer.observe(host.current);
    return () => observer.disconnect();
  }, []);
  return <div className="face-photo__preview" ref={host}>
    {visible ? <LoadedPhotoPreview {...props} /> : null}
  </div>;
}

/** Unmounting an offscreen consumer cancels waiting work and releases its lease. */
function LoadedPhotoPreview({ path, faces, selected, onSelect, unavailable }: Props) {
  const [loaded, setLoaded] = useState<string>();
  const [failed, setFailed] = useState<string>();
  const preview = useQuery({
    queryKey: ["face-photo-preview", path],
    queryFn: async ({ signal }) => {
      const result = await faceWorkbenchPreview(path, signal);
      if (signal.aborted && result?.resource) releaseUnretainedMediaResource(result.resource.resourceId);
      signal.throwIfAborted();
      return result ?? null;
    },
    staleTime: 0, gcTime: 0, retry: 1,
  });
  const result = preview.data;
  const resourceId = result?.resource?.resourceId;
  useEffect(() => {
    if (!resourceId) return;
    const release = retainMediaResource(resourceId);
    let disposed = false;
    let busy = false;
    const renew = async () => {
      if (busy) return;
      busy = true;
      const live = await renewMediaResource(resourceId).catch(() => false);
      if (!live && !disposed) await preview.refetch();
      busy = false;
    };
    void renew();
    const timer = window.setInterval(() => void renew(), 10_000);
    return () => { disposed = true; clearInterval(timer); release(); };
  }, [resourceId, preview.refetch]);
  const geometry = result && validPreviewGeometry(result.geometry, result);
  const size = geometry?.displaySize ?? result;
  const ratio = size ? size.width / size.height : 1.5;
  const styles = geometry && result ? previewContentStyles(geometry, result) : undefined;
  return <>
    {result && failed !== result.url ? <div className="face-photo__canvas" style={{ width: `min(100%, ${ratio * 100}cqh)`, height: `min(100%, ${100 / ratio}cqw)` }}>
      <img alt={path.split(/[\\/]/).at(-1)} src={result.url} style={styles?.image}
        decoding="async" onLoad={() => setLoaded(result.url)} onError={() => setFailed(result.url)} />
      {loaded === result.url ? <RegionOverlay className="face-photo__regions"
        descriptor={{ id: path, coordinateSpace: "displayNormalized", items: faces.map((face) => ({ id: face.observationId, rect: face.bbox, state: face.state, label: face.confirmedPersonName })) }}
        regionClassName={(region) => `face-photo__region${selected.includes(region.id) ? " is-selected" : ""}`}
        renderRegion={(region) => <button type="button" aria-label={region.label ?? `#${faces.findIndex((face) => face.observationId === region.id) + 1}`}
          aria-pressed={selected.includes(region.id)} onClick={(event) => onSelect(region.id, event.ctrlKey || event.metaKey, event.shiftKey)} />}
      /> : null}
    </div> : <span className="face-photo__placeholder" role="status">{preview.isPending ? "…" : unavailable}</span>}
  </>;
}
