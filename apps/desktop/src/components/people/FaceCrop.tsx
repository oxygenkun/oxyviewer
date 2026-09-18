import { useEffect, useMemo, useRef, useState } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";

import { getFaceCrops, renewMediaResource } from "@/lib/api";
import { mediaProtocolUrl } from "@/lib/media/mediaProtocolUrl";
import { releaseUnretainedMediaResource, retainMediaResource } from "@/lib/cache/mediaResourceLease";
import type { FaceCropResource } from "@/types";

/** How often a displayed crop's Host lease is extended. */
const CROP_LEASE_RENEW_MS = 10_000;
const CROP_BATCH_SIZE = 1;

/**
 * Face crops for a set of observations.
 *
 * Crops are requested independently so completed faces appear
 * while the remaining Full images are still decoding. They are held as leased
 * media resources. A resource is released when it is no
 * longer displayed, so the Host can evict the encoded JPEG without the panel
 * having to know about the cache.
 *
 * Registering a crop leases it for the Host's publish grace only, and a local
 * retain does not extend that lease. Without the renewal below, coming back to
 * a tab could render cached URLs of resources the Host has already dropped.
 * Renewal failures re-register those handles while the tab stays open.
 */
export function useFaceCrops(observationIds: string[], size = 128) {
  const queryClient = useQueryClient();
  // React Query keys are structural; sorting keeps the same member set from
  // re-fetching just because the cluster's order changed.
  const key = useMemo(() => [...new Set(observationIds)].sort(), [observationIds]);
  const memberKey = key.join("\u0000");
  const batches = useMemo(() => {
    const result: string[][] = [];
    for (let index = 0; index < key.length; index += CROP_BATCH_SIZE) {
      result.push(key.slice(index, index + CROP_BATCH_SIZE));
    }
    return result;
  }, [memberKey]);
  // One recovery per member set: refetching again on the fresh descriptors
  // would only loop while the Host keeps refusing to register them.
  const recoveredKey = useRef<string | null>(null);
  const queries = useQueries({
    queries: batches.map((batch) => ({
      queryKey: ["face-crops", size, ...batch],
      queryFn: async ({ signal }) => {
        const crops = await getFaceCrops(batch, size);
        if (signal.aborted) {
          for (const crop of crops) releaseUnretainedMediaResource(crop.descriptor.resourceId);
          throw new DOMException("Face crop request cancelled", "AbortError");
        }
        if (crops.length === 0) throw new Error(`Face crop unavailable: ${batch[0]}`);
        return crops;
      },
      staleTime: 30_000,
      retry: 1,
    })),
  });
  const resources = queries.flatMap((query) => query.data ?? []);
  const resourceKey = resources.map((crop) => crop.descriptor.resourceId).join("\u0000");

  useEffect(() => {
    if (resources.length === 0) return;
    const releases = resources.map((crop) => retainMediaResource(crop.descriptor.resourceId));
    let disposed = false;
    const keepAlive = async () => {
      const live = await Promise.all(
        resources.map((crop) => renewMediaResource(crop.descriptor.resourceId).catch(() => false)),
      );
      if (disposed) return;
      if (live.every(Boolean)) {
        // A live set earned its recovery back for the next time it dies.
        recoveredKey.current = null;
        return;
      }
      // A dropped handle cannot be revived by retrying its immutable URL, so
      // ask the Host to register the set again.
      if (recoveredKey.current === memberKey) return;
      recoveredKey.current = memberKey;
      await Promise.all(batches.map((batch) =>
        queryClient.invalidateQueries({ queryKey: ["face-crops", size, ...batch], exact: true }),
      ));
    };
    void keepAlive();
    const timer = window.setInterval(() => void keepAlive(), CROP_LEASE_RENEW_MS);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      releases.forEach((release) => release());
      // A crop that never got a lease (the query resolved after unmount) must
      // still be handed back.
      for (const crop of resources) {
        releaseUnretainedMediaResource(crop.descriptor.resourceId);
      }
    };
  }, [resourceKey, memberKey, queryClient, size]);

  const byObservation = useMemo(() => {
    const map = new Map<string, FaceCropResource>();
    for (const crop of resources) {
      map.set(crop.observationId, crop);
    }
    return map;
  }, [resourceKey]);

  return { byObservation, isLoading: queries.some((query) => query.isLoading) };
}

interface FaceCropProps {
  crop?: FaceCropResource;
  /** Accessible description; the panel supplies the person or asset name. */
  label: string;
  size?: number;
}

/** One square face crop, or a neutral placeholder while it loads. */
export function FaceCrop({ crop, label, size = 48 }: FaceCropProps) {
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [crop?.descriptor.url]);

  if (!crop || failed) {
    return (
      <span
        aria-hidden="true"
        className="face-crop is-placeholder"
        style={{ width: size, height: size }}
      />
    );
  }
  return (
    <img
      alt={label}
      className="face-crop"
      decoding="async"
      height={size}
      onError={() => setFailed(true)}
      src={mediaProtocolUrl(crop.descriptor.url)}
      width={size}
    />
  );
}

interface RevealableFaceCropProps extends FaceCropProps {
  /** True for a cluster's boundary member, which is drawn differently. */
  outlier?: boolean;
  /** Clicking a crop shows the whole photo in the main window's loupe. */
  onReveal: () => void;
  revealTitle: string;
}

/**
 * A face crop that behaves like a link to its photo.
 *
 * The workbench's job is judging faces, but the judgement often needs the
 * context around the face, so one click must reach the photo instead of only
 * confirming from a 64px square.
 */
export function RevealableFaceCrop({
  crop,
  label,
  size,
  outlier = false,
  onReveal,
  revealTitle,
}: RevealableFaceCropProps) {
  return (
    <button
      aria-label={revealTitle}
      className={`face-crop-frame face-crop-frame--action${outlier ? " is-outlier" : ""}`}
      onClick={onReveal}
      title={revealTitle}
      type="button"
    >
      <FaceCrop crop={crop} label={label} size={size} />
    </button>
  );
}
