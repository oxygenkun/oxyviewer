import { useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Check, Trash2, Users } from "lucide-react";

import {
  deletePerson,
  getPersonUndo,
  listCustomTags,
  mergePersons,
  renamePerson,
  undoPersonOperation,
} from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import type { Person, UndoableOperation } from "@/types";
import { RevealableFaceCrop, useFaceCrops } from "./FaceCrop";
import { PersonTagPathEditor } from "./PersonTagPathEditor";

function undoLabel(operation: UndoableOperation, t: (key: MessageKey) => string): string {
  if (operation.kind === "mergePersons") {
    return t("peopleUndoMerge")
      .replace("{name}", operation.otherPersonName ?? "")
      .replace("{count}", String(operation.faceCount));
  }
  if (operation.kind === "assignFaces") {
    return t("peopleUndoAssign").replace("{count}", String(operation.faceCount));
  }
  return t("peopleUndoDetach").replace("{count}", String(operation.faceCount));
}

interface PersonManagerProps {
  invalidate: () => void;
  onReveal: (observationId: string) => void;
  open: boolean;
  persons: Person[];
  revealTitle: string;
  setOpen: (open: boolean) => void;
  t: (key: MessageKey) => string;
}

export function PersonManager({
  invalidate,
  onReveal,
  open,
  persons,
  revealTitle,
  setOpen,
  t,
}: PersonManagerProps) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [mergeSource, setMergeSource] = useState("");
  const tags = useQuery({ queryKey: ["custom-tags"], queryFn: listCustomTags });
  const undoState = useQuery({ queryKey: ["face-undo"], queryFn: getPersonUndo });

  const removePerson = useMutation({ mutationFn: deletePerson, onSuccess: invalidate });
  const merge = useMutation({
    mutationFn: ({ source, target }: { source: string; target: string }) => mergePersons(source, target),
    onSuccess: () => {
      setMergeSource("");
      invalidate();
    },
  });
  const undo = useMutation({ mutationFn: undoPersonOperation, onSuccess: invalidate });
  const rename = useMutation({
    mutationFn: ({ personId, name }: { personId: string; name: string }) => renamePerson(personId, name),
    onSuccess: () => {
      setRenaming(null);
      invalidate();
    },
  });

  const tagPaths = useMemo(
    () => new Map((tags.data ?? []).map((tag) => [tag.id, tag.path])),
    [tags.data],
  );
  const personPath = (person: Person) =>
    tagPaths.get(person.linkedTagId ?? -1) ?? `人物|${person.displayName}`;
  const personList = useMemo(
    () => [...persons].sort((a, b) => personPath(a).localeCompare(personPath(b))),
    [persons, tagPaths],
  );
  const coverIds = useMemo(
    () => persons.flatMap((person) => person.coverObservationId ? [person.coverObservationId] : []),
    [persons],
  );
  const covers = useFaceCrops(open ? coverIds : []);

  return (
    <>
      <div className="face-photos__management">
        <button aria-expanded={open} onClick={() => setOpen(!open)} type="button">
          <Users size={15} /> {t("peoplePersons")}
        </button>
        {undoState.data ? (
          <button disabled={undo.isPending} onClick={() => undo.mutate()} type="button">
            ↶ {undoLabel(undoState.data, t)}
          </button>
        ) : null}
      </div>
      {open ? (
        <section className="face-workbench__section">
          <header className="face-workbench__section-head">
            <h2>{t("peoplePersons")}</h2>
            <p>{t("faceWorkbenchPersonsHint")}</p>
          </header>
          {personList.length ? (
            <ul className="people-panel__persons">
              {personList.map((person) => (
                <li key={person.personId}>
                  {person.coverObservationId ? (
                    <RevealableFaceCrop
                      crop={covers.byObservation.get(person.coverObservationId)}
                      label={person.displayName}
                      onReveal={() => onReveal(person.coverObservationId!)}
                      revealTitle={revealTitle}
                      size={42}
                    />
                  ) : null}
                  {renaming === person.personId ? (
                    <>
                      <input
                        autoFocus
                        onChange={(event) => setRenameValue(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" && renameValue.trim()) {
                            rename.mutate({ personId: person.personId, name: renameValue });
                          }
                          if (event.key === "Escape") setRenaming(null);
                        }}
                        value={renameValue}
                      />
                      <button
                        aria-label={t("peopleRenameConfirm")}
                        disabled={!renameValue.trim() || rename.isPending}
                        onClick={() => rename.mutate({ personId: person.personId, name: renameValue })}
                        type="button"
                      >
                        <Check size={14} />
                      </button>
                    </>
                  ) : (
                    <>
                      <button
                        className="people-panel__person-name"
                        onClick={() => {
                          setRenaming(person.personId);
                          setRenameValue(person.displayName);
                        }}
                        type="button"
                      >
                        {person.displayName}
                      </button>
                      <span className="people-panel__badge">
                        {t("peoplePersonFaces").replace("{count}", String(person.faceCount))}
                      </span>
                      <button
                        aria-label={t("peopleDeletePerson").replace("{name}", person.displayName)}
                        onClick={() => removePerson.mutate(person.personId)}
                        type="button"
                      >
                        <Trash2 size={14} />
                      </button>
                    </>
                  )}
                  <PersonTagPathEditor person={person} path={personPath(person)} t={t} onSaved={invalidate} />
                </li>
              ))}
            </ul>
          ) : (
            <p className="people-panel__empty">{t("peopleNoPersons")}</p>
          )}
          {personList.length > 1 ? (
            <label className="people-panel__field">
              <span>{t("peopleMergeHint")}</span>
              <select onChange={(event) => setMergeSource(event.target.value)} value={mergeSource}>
                <option value="">{t("peopleMergeChoose")}</option>
                {personList.map((person) => (
                  <option key={person.personId} value={person.personId}>{person.displayName}</option>
                ))}
              </select>
              <select
                disabled={!mergeSource}
                onChange={(event) => {
                  if (event.target.value) merge.mutate({ source: mergeSource, target: event.target.value });
                }}
                value=""
              >
                <option value="">{t("peopleMergeInto")}</option>
                {personList.filter((person) => person.personId !== mergeSource).map((person) => (
                  <option key={person.personId} value={person.personId}>{person.displayName}</option>
                ))}
              </select>
            </label>
          ) : null}
        </section>
      ) : null}
    </>
  );
}
