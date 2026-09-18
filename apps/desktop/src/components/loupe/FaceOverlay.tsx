import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { createPerson, decideFace, getAssetFaceReviews } from "@/lib/api";
import { mediaProtocolUrl } from "@/lib/media/mediaProtocolUrl";
import { useFaceCrops } from "@/components/people/FaceCrop";
import type { AssetSummary, FaceReviewItem } from "@/types";
import type { MessageKey } from "@/lib/i18n";

interface FaceOverlayProps {
  asset: AssetSummary;
  t: (key: MessageKey) => string;
  /** Only the asset on screen is queried; the filmstrip does not need boxes. */
  enabled: boolean;
}

/**
 * Face boxes over the loupe image, with in-place confirmation.
 *
 * This is the shortest correction path the feature has: the user sees the face
 * and the proposed name at the same time, so "correct" is a click on the photo
 * rather than a trip to a separate review list. Boxes are drawn from the same
 * normalized regions the analyzer stored, so they stay correct at any zoom.
 */
export function FaceOverlay({ asset, t, enabled }: FaceOverlayProps) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");

  const reviews = useQuery({
    queryKey: ["asset-face-reviews", asset.path],
    queryFn: () => getAssetFaceReviews(asset.path),
    enabled,
    staleTime: 5_000,
  });

  // A face crop for the selected box makes the label unambiguous when two
  // people look similar. The hook owns the resource leases, including the
  // renewal that keeps the crops alive across a tab switch.
  const observationIds = enabled ? reviews.data?.map((item) => item.observationId) ?? [] : [];
  const crops = useFaceCrops(observationIds, 96);

  useEffect(() => {
    setSelected(null);
    setDraftName("");
  }, [asset.path]);

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["asset-face-reviews", asset.path] });
    void queryClient.invalidateQueries({ queryKey: ["face-review"] });
    void queryClient.invalidateQueries({ queryKey: ["face-persons"] });
    void queryClient.invalidateQueries({ queryKey: ["face-capability"] });
  };

  const decide = useMutation({
    mutationFn: ({ observationId, decision }: { observationId: string; decision: Parameters<typeof decideFace>[1] }) =>
      decideFace(observationId, decision),
    onSuccess: invalidate,
  });
  const nameAndConfirm = useMutation({
    mutationFn: async ({ observationId, name }: { observationId: string; name: string }) => {
      const person = await createPerson(crypto.randomUUID(), name);
      await decideFace(observationId, { decision: "confirmPerson", personId: person.personId });
      return person;
    },
    onSuccess: () => {
      setDraftName("");
      invalidate();
    },
  });

  if (!enabled || !reviews.data?.length) return null;

  const label = (item: FaceReviewItem) => {
    if (item.confirmedPersonName) return item.confirmedPersonName;
    if (item.candidate) {
      return t("faceOverlayCandidate")
        .replace("{name}", item.candidate.personName)
        .replace("{value}", item.candidate.similarity.toFixed(2));
    }
    return t("faceOverlayUnknown");
  };

  return (
    <div className="loupe__face-overlay">
      {reviews.data.map((item) => {
        const isSelected = selected === item.observationId;
        const crop = crops.byObservation.get(item.observationId);
        return (
          <div
            className={`loupe__face-box is-${item.state}${isSelected ? " is-selected" : ""}`}
            data-testid={`face-box-${item.observationId}`}
            key={item.observationId}
            style={{
              left: `${item.bbox.x * 100}%`,
              top: `${item.bbox.y * 100}%`,
              width: `${item.bbox.width * 100}%`,
              height: `${item.bbox.height * 100}%`,
            }}
          >
            <button
              className="loupe__face-anchor"
              onClick={() =>
                setSelected(isSelected ? null : item.observationId)
              }
              title={`${label(item)} · ${t("faceOverlayDetection").replace("{value}", item.detectionScore.toFixed(2))}`}
              type="button"
            >
              <span className="loupe__face-label">{label(item)}</span>
            </button>
            {isSelected ? (
              <div className="loupe__face-actions" onClick={(event) => event.stopPropagation()}>
                {crop ? (
                  <img
                    alt={label(item)}
                    className="loupe__face-crop"
                    src={mediaProtocolUrl(crop.descriptor.url)}
                  />
                ) : null}
                {item.candidate ? (
                  <>
                    <button
                      disabled={decide.isPending}
                      onClick={() =>
                        decide.mutate({
                          observationId: item.observationId,
                          decision: {
                            decision: "confirmPerson",
                            personId: item.candidate!.personId,
                          },
                        })
                      }
                      title={t("peopleConfirmMatch")}
                      type="button"
                    >
                      ✓ {item.candidate.personName}
                    </button>
                    <button
                      disabled={decide.isPending}
                      onClick={() =>
                        decide.mutate({
                          observationId: item.observationId,
                          decision: {
                            decision: "rejectPerson",
                            personId: item.candidate!.personId,
                          },
                        })
                      }
                      title={t("peopleRejectMatch")}
                      type="button"
                    >
                      ✗ {t("faceOverlayNotThisPerson")}
                    </button>
                  </>
                ) : (
                  <form
                    onSubmit={(event) => {
                      event.preventDefault();
                      const name = draftName.trim();
                      if (!name) return;
                      nameAndConfirm.mutate({ observationId: item.observationId, name });
                    }}
                  >
                    <input
                      aria-label={t("peopleNamePlaceholder")}
                      onChange={(event) => setDraftName(event.target.value)}
                      placeholder={t("peopleNamePlaceholder")}
                      value={draftName}
                    />
                    <button disabled={!draftName.trim() || nameAndConfirm.isPending} type="submit">
                      {t("faceOverlayNameAndConfirm")}
                    </button>
                  </form>
                )}
                <button
                  disabled={decide.isPending}
                  onClick={() =>
                    decide.mutate({
                      observationId: item.observationId,
                      decision: { decision: "notFace" },
                    })
                  }
                  title={t("peopleNotFace")}
                  type="button"
                >
                  ⊘ {t("peopleNotFace")}
                </button>
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
