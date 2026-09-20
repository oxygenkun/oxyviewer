import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { setPersonTagPath } from "@/lib/api";
import { parseTagPath } from "@/lib/assets/tagTree";
import type { MessageKey } from "@/lib/i18n";
import type { Person } from "@/types";

export function PersonTagPathEditor({ person, path, t, onSaved }: {
  person: Person; path: string; t: (key: MessageKey) => string; onSaved: () => void;
}) {
  const [draft, setDraft] = useState<string>();
  const mutation = useMutation({
    mutationFn: (value: string) => setPersonTagPath(person.personId, parseTagPath(value).join("|")),
    onSuccess: () => { setDraft(undefined); onSaved(); },
  });
  const value = draft ?? path.replaceAll("|", " / ");
  return <form className="people-person-path" onSubmit={(event) => { event.preventDefault(); mutation.mutate(value); }}>
    <label>{t("peopleTagPath")}<input aria-label={`${t("peopleTagPath")} · ${person.displayName}`}
      placeholder={t("peopleTagPathExample")} value={value} disabled={mutation.isPending}
      onChange={(event) => setDraft(event.target.value)} /></label>
    <button type="submit" disabled={mutation.isPending || !parseTagPath(value).length || parseTagPath(value).join("|") === path}>{t("peopleTagPathSave")}</button>
    {mutation.isError ? <span role="alert">{String(mutation.error)}</span> : null}
  </form>;
}
