import type { MessageKey } from "@/lib/i18n";
import { undoTagDeletion, useTagDeletions } from "@/lib/assets/tagDeletion";
import { Toast } from "./Toast";

export function TagDeletionToasts({ t }: { t: (key: MessageKey) => string }) {
  const requests = useTagDeletions(state => state.requests);
  return requests.filter(request => request.status !== "committing").map(request => {
    const failed = request.status === "failed";
    const failureKey: MessageKey = request.kind === "tag" ? "tagDeletionFailed" : "tagRemovalFailed";
    return (
      <Toast
        key={request.id}
        tone={failed ? "danger" : "pending"}
        actionLabel={t(failed ? "cancel" : "undoDeletion")}
        onAction={() => undoTagDeletion(request.id)}
      >
        {failed ? `${t(failureKey)}: ${request.error}`
          : t(request.kind === "tag" ? "tagDeletionPending" : "tagRemovalPending")
            .replace("{tag}", request.tagPath.replaceAll("|", " › "))}
      </Toast>
    );
  });
}
