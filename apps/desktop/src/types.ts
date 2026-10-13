// Rust IPC definitions come from the checked generator; UI-only types stay here.
export type * from './generated/contracts';
import type * as Native from './generated/contracts';

export type PersonTupleFilter = Omit<Native.PersonTupleFilter, "decision"> & {
  decision?: Native.PersonTupleFilter["decision"];
};

/** Query controls before Rust applies its serde defaults. */
type QueryFields = Omit<Native.AssetQuery, "personTupleFilter"> & { personTupleFilter: PersonTupleFilter | null };
export type AssetQuery = Pick<QueryFields, "sort" | "direction"> & {
  [K in Exclude<keyof QueryFields, "sort" | "direction">]?: NonNullable<QueryFields[K]>;
} & { pageSize: number };

/** UI paging also carries optional browse diagnostics and snapshot fencing. */
export type Page<T> = Omit<Native.Page<T>, "nextCursor"> & {
  nextCursor?: Native.Page<T>["nextCursor"];
} & Partial<Omit<Native.BrowsePage, keyof Native.Page<T>>>;

/** A browser URL is attached after the native resource descriptor is resolved. */
export type PreviewResult = Omit<Native.PreviewResult, "geometry"> & {
  url: string;
  geometry?: NonNullable<Native.PreviewResult["geometry"]>;
};

/** The tag tree uses an absent parent as its local root key. */
export type CustomTag = Omit<Native.CustomTag, "parentId"> & { parentId?: number };
export type AssetTagAssignment = Omit<Native.AssetTagAssignment, "tag"> & { tag: CustomTag };
export type AssetTagAssignmentsByPath = Omit<Native.AssetTagAssignmentsByPath, "assignments"> & {
  assignments: AssetTagAssignment[];
};


export type ViewMode = "grid" | "loupe";
export type ThumbnailOrientation = "landscape" | "portrait";
export type NavigatorPosition = "top-left" | "top-right" | "bottom-left" | "bottom-right";

export interface WindowDragPosition {
  x: number;
  y: number;
}

/** Normalized native drag-and-drop payload from the desktop window. */
export type WindowDragEvent =
  | { type: "enter"; paths: string[]; position: WindowDragPosition }
  | { type: "over"; position: WindowDragPosition }
  | { type: "drop"; paths: string[]; position: WindowDragPosition }
  | { type: "leave" };

export interface PerfMark {
  name: string;
  /** Milliseconds relative to page load, from performance.now(). */
  t: number;
  detail?: Record<string, unknown>;
}
export type { PersonBox } from "@/lib/peopleContracts";
