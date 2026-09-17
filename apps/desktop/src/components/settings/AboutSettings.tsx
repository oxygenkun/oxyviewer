import { Check, ExternalLink, Info, LoaderCircle, RefreshCw } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { checkForUpdates, getAppInfo, openAboutLink } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { UpdateStatus } from "@/types";

/** How long the transient "already up to date" confirmation stays on screen. */
export const UP_TO_DATE_DISMISS_MS = 6000;

/** GitHub timestamps are ISO-8601; only the calendar date is worth showing. */
function releaseDate(publishedAt: string): string {
  const parsed = new Date(publishedAt);
  return Number.isNaN(parsed.valueOf()) ? publishedAt : parsed.toISOString().slice(0, 10);
}

export function AboutSettings({ t }: { t: (key: MessageKey) => string }) {
  const appInfo = useQuery({
    queryKey: ["app-info"],
    queryFn: getAppInfo,
    staleTime: Infinity,
  });
  const [status, setStatus] = useState<UpdateStatus>();
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string>();

  const info = appInfo.data;

  const runCheck = async () => {
    setChecking(true);
    setError(undefined);
    try {
      setStatus(await checkForUpdates());
    } catch (cause) {
      setStatus(undefined);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setChecking(false);
    }
  };

  const releaseMeta = status?.updateAvailable
    ? [status.releaseName, status.publishedAt ? releaseDate(status.publishedAt) : undefined]
        .filter(Boolean)
        .join(" · ")
    : "";

  // Confirming "up to date" is only worth a moment; an available update stays
  // until the panel is left, because it asks the user to act.
  const confirmingCurrent = status !== undefined && !status.updateAvailable;
  useEffect(() => {
    if (!confirmingCurrent) return;
    const timer = window.setTimeout(() => setStatus(undefined), UP_TO_DATE_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [confirmingCurrent]);

  return (
    <>
      <section className="settings-panel__section">
        <div className="about-settings__identity">
          <span className="about-settings__mark" aria-hidden="true"><Info size={17} /></span>
          <div className="about-settings__title">
            <strong>{info?.name ?? "OxyViewer"}</strong>
            <span className="about-settings__version">
              {`${t("aboutVersion")} ${info?.version ?? t("loading")}`}
              <button disabled={checking} onClick={() => void runCheck()} type="button">
                {checking ? <LoaderCircle className="is-spinning" size={13} /> : <RefreshCw size={13} />}
                {checking ? t("aboutChecking") : t("aboutCheckUpdates")}
              </button>
              {status ? (
                status.updateAvailable ? (
                  <span className="about-settings__result is-available">
                    <span>{t("aboutUpdateAvailable").replace("{version}", status.latestVersion)}</span>
                    <button onClick={() => void openAboutLink("releases", status.releaseUrl)} type="button">
                      {t("aboutOpenRelease")}<ExternalLink size={12} />
                    </button>
                  </span>
                ) : (
                  <span className="about-settings__result" role="status">
                    <Check size={13} />
                    {t("aboutUpToDate")}
                  </span>
                )
              ) : null}
            </span>
          </div>
        </div>
        {releaseMeta ? <p className="settings-panel__hint">{releaseMeta}</p> : null}
        {error ? <p className="settings-panel__error">{error}</p> : null}
      </section>

      <section className="settings-panel__section">
        <dl className="about-settings__facts">
          <div>
            <dt>{t("aboutRepository")}</dt>
            <dd>
              <button className="about-settings__link" onClick={() => void openAboutLink("repository")} type="button">
                {info?.repositoryUrl ?? t("loading")}
                <ExternalLink size={12} />
              </button>
            </dd>
          </div>
          <div>
            <dt>{t("aboutAuthor")}</dt>
            <dd>{info?.author ?? t("unavailable")}</dd>
          </div>
          <div>
            <dt>{t("aboutLicense")}</dt>
            <dd>
              <button
                className="about-settings__link"
                onClick={() => void openAboutLink("license")}
                title={info?.license ?? undefined}
                type="button"
              >
                AGPL
                <ExternalLink size={12} />
              </button>
            </dd>
          </div>
        </dl>
      </section>
    </>
  );
}
