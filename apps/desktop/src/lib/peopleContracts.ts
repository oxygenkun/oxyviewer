import type { AssetSummary, PersonReviewDecision } from "@/types";
export type PersonBox = [number, number, number, number];
export interface GlobalPerson { id: string; displayName: string; revision: number; referenceInstanceIds: string[]; tagId: number | null }
export interface GlobalPersonGallery { tuples: PersonTuple[]; total: number; folderCount: number }
export interface GlobalPersonFolder { folderPath: string; coverAssetPath: string; photoCount: number; instanceCount: number }
export interface GlobalPersonFolders { folders: GlobalPersonFolder[]; total: number }
export interface PersonTuple { id: string; assetPath: string; sourceRevision: string; sourceIdentityRevision: string; faceBox: PersonBox | null; bodyBox: PersonBox | null; revision: number; needsReview: boolean; personId: string | null; decision: PersonReviewDecision | null; score: number | null }
export interface PersonTupleGroup { id: string; personId: string | null; members: PersonTuple[]; cover: AssetSummary | null }
export interface FolderPeopleWorkspace { folderPath: string; revision: string; groups: PersonTupleGroup[]; unknownCount: number; knownCount: number; noFaceCount: number; unavailableCount: number; hasAnalysis: boolean; notice: string | null }
export interface PersonTupleFilter { groupId: string; decision?: PersonReviewDecision | null }
