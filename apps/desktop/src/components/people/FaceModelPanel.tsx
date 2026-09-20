import { Check, Download, Loader2 } from "lucide-react";

import type { FaceModelDownloadProgress, FaceModelStatus } from "@/types";
import type { MessageKey } from "@/lib/i18n";

interface FaceModelPanelProps {
  available: boolean;
  error?: unknown;
  installing: boolean;
  installingId?: string;
  models: FaceModelStatus[];
  onInstall: (modelId: string) => void;
  progress?: FaceModelDownloadProgress;
  running: boolean;
  t: (key: MessageKey) => string;
}

export function FaceModelPanel({
  available,
  error,
  installing,
  installingId,
  models,
  onInstall,
  progress,
  running,
  t,
}: FaceModelPanelProps) {
  return (
    <section className="face-launch__block face-models">
      <div className="face-models__heading">
        <span>{t("peopleModels")}</span>
        <small>{available ? t("peopleModelsReady") : t("peopleModelsRequired")}</small>
      </div>
      {models.map((model) => {
        const modelProgress = progress?.modelId === model.id && !model.installed ? progress : undefined;
        const percent = modelProgress && modelProgress.totalBytes > 0
          ? Math.min(100, modelProgress.downloadedBytes / modelProgress.totalBytes * 100)
          : 0;
        return (
          <div className="face-models__item" key={model.id}>
            <div>
              <strong>{model.displayName}</strong>
              <small>
                {(model.downloadSizeBytes / 1024 / 1024).toFixed(0)} MB {t("peopleModelDownloadSize")} · {model.licenseSummary}
              </small>
              {modelProgress ? (
                <div
                  aria-valuemax={100}
                  aria-valuemin={0}
                  aria-valuenow={Math.round(percent)}
                  className="face-models__progress"
                  role="progressbar"
                >
                  <span className="face-models__progress-label">
                    {t(`peopleModelStage_${modelProgress.stage}` as MessageKey)}
                    {modelProgress.stage === "downloading" ? ` ${Math.round(percent)}%` : ""}
                  </span>
                  <span aria-hidden="true" className="face-models__progress-track">
                    <span style={{ width: `${percent}%` }} />
                  </span>
                </div>
              ) : null}
            </div>
            {model.installed ? (
              <span className="face-models__installed"><Check size={13} /> {t("peopleModelInstalled")}</span>
            ) : (
              <button disabled={running || installing} onClick={() => onInstall(model.id)} type="button">
                {installing && installingId === model.id
                  ? <Loader2 className="is-spinning" size={14} />
                  : <Download size={14} />}
                {t("peopleModelDownload")}
              </button>
            )}
          </div>
        );
      })}
      {error ? <p className="face-launch__summary is-error" role="alert">{String(error)}</p> : null}
      <p className="people-panel__hint">{t("peopleModelsHint")}</p>
    </section>
  );
}
