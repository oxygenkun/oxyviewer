import { FolderDown } from "lucide-react";
import type { MessageKey } from "../lib/i18n";

interface ImportOverlayProps {
  visible: boolean;
  folderNames: string[];
  itemCount: number;
  t: (key: MessageKey) => string;
}

/**
 * Drop affordance shown while an OS drag hovers the window. It is purely visual
 * and covers the window with a translucent card, so `pointer-events: none` is
 * what keeps the drop itself landing on the app.
 */
export function ImportOverlay({ visible, folderNames, itemCount, t }: ImportOverlayProps) {
  const hiddenCount = Math.max(itemCount - folderNames.length, 0);
  return (
    <div className={`drop-overlay ${visible ? "is-visible" : ""}`} aria-hidden="true">
      <div className="drop-overlay__card">
        <FolderDown size={34} strokeWidth={1.4} />
        <strong>{t("dropToImport")}</strong>
        <span>{t("dropToImportHint")}</span>
        {folderNames.length ? (
          <ul className="drop-overlay__names">
            {folderNames.map((name) => <li key={name}>{name}</li>)}
            {hiddenCount > 0 ? <li>+{hiddenCount}</li> : null}
          </ul>
        ) : null}
      </div>
    </div>
  );
}
