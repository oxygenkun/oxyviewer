import { Field } from "../ui/Field";
import { Button } from "../ui/Button";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { chooseExternalApplication, isTauri, updateExternalAppSettings } from "@/lib/api";
import { EXTERNAL_APPS_QUERY_KEY, moveExternalApplication, removeExternalApplication, useExternalAppSettings } from "@/lib/media/externalApps";
import type { MessageKey } from "@/lib/i18n";
import type { ExternalApplication, ExternalAppSettings } from "@/types";

export function ExternalAppsSettings({ t, focus }: { t: (key: MessageKey) => string; focus: boolean }) {
  const query = useExternalAppSettings();
  const client = useQueryClient();
  const section = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState<ExternalApplication>();
  const [dialogError, setDialogError] = useState<string>();
  const [choosing, setChoosing] = useState(false);
  const save = useMutation({
    mutationFn: updateExternalAppSettings,
    onSuccess: (settings) => { client.setQueryData(EXTERNAL_APPS_QUERY_KEY, settings); setDraft(undefined); },
  });
  useEffect(() => { if (focus) section.current?.scrollIntoView({ block: "start" }); }, [focus]);
  const settings = query.data;
  const busy = save.isPending || choosing;
  const choose = async (existing?: ExternalApplication) => {
    setDialogError(undefined);
    setChoosing(true);
    try {
      const path = await chooseExternalApplication();
      if (path) setDraft({ id: existing?.id ?? crypto.randomUUID(), name: existing?.name ?? path.split(/[\\/]/).pop()!.replace(/\.exe$/i, ""), executablePath: path });
    } catch (error) { setDialogError(String(error)); }
    finally { setChoosing(false); }
  };
  const commit = () => {
    if (!draft || !settings) return;
    const app = { ...draft, name: draft.name.trim() };
    const apps = settings.apps.some((entry) => entry.id === app.id)
      ? settings.apps.map((entry) => entry.id === app.id ? app : entry) : [...settings.apps, app];
    save.mutate({ apps, defaultAppId: settings.defaultAppId ?? app.id });
  };
  const update = (next: ExternalAppSettings) => { save.reset(); save.mutate(next); };
  return <div className="settings-panel__section external-apps-settings" ref={section}>
    <div className="settings-panel__section-heading">
      <span className="settings-panel__label">{t("externalApps")}</span>
      <Button size="small" disabled={busy || !settings || !!draft || !isTauri()} onClick={() => { save.reset(); void choose(); }}>{t("externalAdd")}</Button>
    </div>
    <p className="external-apps-settings__hint">{t("externalDefaultHint")}</p>
    {!isTauri() ? <p className="external-apps-settings__hint">{t("externalDesktopOnly")}</p> : null}
    {query.isPending ? <p>{t("externalLoading")}</p> : null}
    {settings?.apps.length === 0 ? <p className="external-apps-settings__hint">{t("externalEmpty")}</p> : null}
    {settings?.apps.map((app, index) => <div className="external-apps-settings__row" key={app.id}>
      <label>
        <input type="radio" name="external-default" checked={app.id === settings.defaultAppId} disabled={busy || !!draft}
          onChange={() => update({ ...settings, defaultAppId: app.id })} aria-label={t("externalSetDefault").replace("{name}", app.name)} />
        <strong>{app.name}</strong>
      </label>
      <div className="external-apps-settings__actions">
        <Button size="small" disabled={busy || !!draft} onClick={() => { save.reset(); setDraft({ ...app }); }}>{t("externalEdit")}</Button>
        <Button size="small" aria-label={`${t("externalUp")} ${app.name}`} disabled={busy || !!draft || index === 0} onClick={() => update(moveExternalApplication(settings, index, -1))}>{t("externalUp")}</Button>
        <Button size="small" aria-label={`${t("externalDown")} ${app.name}`} disabled={busy || !!draft || index === settings.apps.length - 1} onClick={() => update(moveExternalApplication(settings, index, 1))}>{t("externalDown")}</Button>
        <Button size="small" disabled={busy || !!draft} onClick={() => update(removeExternalApplication(settings, app.id))}>{t("externalRemove")}</Button>
      </div>
      <span className="external-apps-settings__path" title={app.executablePath}>{app.executablePath}</span>
    </div>)}
    {draft ? <fieldset className="external-apps-settings__editor" disabled={busy}>
      <Field label={t("externalName")} autoFocus value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} />
      <Field label={t("externalProgram")} readOnly value={draft.executablePath} />
      <div className="external-apps-settings__actions">
        <Button size="small" onClick={() => void choose(draft)} disabled={!isTauri()}>{t("externalChoose")}</Button>
        <Button size="small" disabled={!draft.name.trim()} onClick={commit}>{t("externalSave")}</Button>
        <Button size="small" onClick={() => { setDraft(undefined); save.reset(); setDialogError(undefined); }}>{t("externalCancel")}</Button>
      </div>
    </fieldset> : null}
    {query.error || save.error || dialogError ? <p role="alert" className="settings-panel__error">{String(dialogError ?? save.error ?? query.error)}</p> : null}
  </div>;
}
