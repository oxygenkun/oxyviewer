import { createContext, useContext, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getFolderPeopleWorkspace, listGlobalPeople } from "@/lib/api";
import type { AssetSummary, PersonTuple, PersonTupleFilter } from "@/types";

export function useGlobalPeople(folderPath: string | undefined, enabled: boolean, filter: PersonTupleFilter | undefined, setFilter: (filter?: PersonTupleFilter) => void) {
  const client = useQueryClient();
  const catalog = useQuery({ queryKey: ["global-people"], queryFn: listGlobalPeople, enabled });
  const result = useQuery({ queryKey: ["people-workspace", folderPath], queryFn: () => getFolderPeopleWorkspace(folderPath!), enabled: enabled && !!folderPath });
  const [libraryVisit, setLibraryVisit] = useState(0);
  const openLibrary = () => { setSurface("library"); setLibraryVisit(value => value + 1); };
  const [surface, setSurface] = useState<"library" | "folder">("folder");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [draw, setDraw] = useState<"face" | "body">();
  const [edit, setEdit] = useState<PersonTuple>();
  const [heldAsset,setHeldAsset]=useState<AssetSummary>();
  const [showBoxes, setShowBoxes] = useState(true);
  const anchor = useRef<string | undefined>(undefined);
  useEffect(() => { setHeldAsset(undefined); setSelected(new Set()); setDraw(undefined); setEdit(undefined); anchor.current = undefined; }, [folderPath, enabled]);
  const refresh = async () => { await Promise.all(["global-people", "global-reference", "global-gallery", "global-person-folders", "people-workspace", "assets", "person-instances", "person-reviews", "asset-tag-assignments", "tag-sync-status"].map(key => client.invalidateQueries({ queryKey: [key] }))); };
  const group = result.data?.groups.find(g => g.id === filter?.groupId);
  const members = group?.members.filter(t => !filter?.decision || t.decision === filter.decision) ?? [];
  const choose = (id: string, additive = false, range = false) => {
    setSelected(old => {
      if (range && anchor.current) { const a = members.findIndex(t => t.id === anchor.current), b = members.findIndex(t => t.id === id); if (a >= 0 && b >= 0) return new Set([...old, ...members.slice(Math.min(a, b), Math.max(a, b) + 1).map(t => t.id)]); }
      if (!additive) return new Set([id]); const next = new Set(old); if (next.has(id)) next.delete(id); else next.add(id); return next;
    }); anchor.current = id;
  };
  const changeFilter = (next?: PersonTupleFilter, retainedAsset?:AssetSummary) => { setHeldAsset(retainedAsset); setSelected(new Set()); setDraw(undefined); setEdit(undefined); anchor.current = undefined; setFilter(next); };
  return { libraryVisit, openLibrary, surface, setSurface, heldAsset, setHeldAsset, folderPath, enabled, catalog, result, workspace: result.data, people: catalog.data ?? [], filter, setFilter: changeFilter, group, members, selected, setSelected, choose, draw, setDraw, edit, setEdit, showBoxes, setShowBoxes, refresh };
}
export type GlobalPeopleState = ReturnType<typeof useGlobalPeople>;
export const GlobalPeopleContext = createContext<GlobalPeopleState | undefined>(undefined);
export const useGlobalPeopleContext = () => useContext(GlobalPeopleContext);

/** Display one tuple per photo person, preferring its current positive assignment. */
export function uniquePersonTuples(tuples: PersonTuple[]): PersonTuple[] {
  const rank = (t: PersonTuple) => t.decision === "belongs" ? 3 : t.decision === "pending" || t.decision === "deferred" ? 2 : t.personId ? 0 : 1;
  const result = new Map<string, PersonTuple>();
  for (const tuple of tuples) { const old = result.get(tuple.id); if (!old || rank(tuple) > rank(old)) result.set(tuple.id, tuple); }
  return [...result.values()];
}
