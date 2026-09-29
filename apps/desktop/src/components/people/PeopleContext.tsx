import { createContext, useContext } from "react";
import type { AssetSummary, FolderPerson, PersonInstance } from "@/types";
export type Box = [number, number, number, number];
export interface PeopleWorkspace {
  folderPath: string;
  person: FolderPerson;
  selectedInstanceId?: string;
  selectInstance: (id?: string) => void;
  draw?: "face" | "body";
  setDraw: (mode?: "face" | "body") => void;
  editInstance?: PersonInstance;
  setEditInstance: (instance?: PersonInstance) => void;
  showBoxes: boolean;
  changed: (asset?: AssetSummary, navigate?: boolean) => Promise<void>;
}
export const PeopleContext = createContext<PeopleWorkspace | undefined>(undefined);
export const usePeopleWorkspace = () => useContext(PeopleContext);

export const PersonDetectionContext = createContext<{ folderPath: string; showBoxes: boolean; adopt: (asset: AssetSummary, sourceRevision: string, instanceId: string) => Promise<void> } | undefined>(undefined);
