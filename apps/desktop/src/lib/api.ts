import type { RootRelocationPlan, RootRelocationResult } from "@/types";
import type { GlobalPerson,PersonTuple,FolderPeopleWorkspace,PersonBox } from "@/types";

const demoGlobalPeople:GlobalPerson[]=[];
const demoGlobalWorkspaces=new Map<string,FolderPeopleWorkspace>();
export async function getGlobalPersonFolders(personId: string, offset = 0, limit = 24): Promise<import("./peopleContracts").GlobalPersonFolders> {
  if (isTauri()) return invoke("get_global_person_folders", { personId, offset, limit });
  const folders = [...demoGlobalWorkspaces.values()].map(w => {
    const tuples = [...new Map(w.groups.flatMap(g => g.members).filter(t => t.personId === personId && t.decision === "belongs").map(t => [t.id, t])).values()];
    return { folderPath: w.folderPath, coverAssetPath: tuples.map(t => t.assetPath).sort()[0] ?? "", photoCount: new Set(tuples.map(t => t.assetPath)).size, instanceCount: tuples.length };
  }).filter(f => f.instanceCount > 0).sort((a, b) => a.folderPath < b.folderPath ? -1 : a.folderPath > b.folderPath ? 1 : 0);
  return { folders: folders.slice(offset, offset + Math.max(1, Math.min(60, limit))), total: folders.length };
}
export async function getGlobalPersonGallery(personId: string, offset = 0, limit = 24): Promise<import("./peopleContracts").GlobalPersonGallery> {
  if (isTauri()) return invoke("get_global_person_gallery", { personId, offset, limit });
  const tuples = [...new Map([...demoGlobalWorkspaces.values()].flatMap(w => w.groups.flatMap(g => g.members)).filter(t => t.personId === personId && t.decision === "belongs").map(t => [t.id, t])).values()];
  return { tuples: structuredClone(tuples.slice(offset, offset + Math.max(1, Math.min(60, limit)))), total: tuples.length, folderCount: new Set(tuples.map(t => t.assetPath.replace(/[\\/][^\\/]+$/, ""))).size };
}
export async function listGlobalPeople():Promise<GlobalPerson[]>{return isTauri()?invoke("list_global_people"):structuredClone(demoGlobalPeople);}
export async function getFolderPeopleWorkspace(folderPath:string):Promise<FolderPeopleWorkspace>{
  if(isTauri())return invoke("get_folder_people_workspace",{folderPath});
  return structuredClone(demoGlobalWorkspaces.get(folderPath)??{folderPath,revision:"demo",groups:[],unknownCount:0,knownCount:0,noFaceCount:0,unavailableCount:0,hasAnalysis:false,notice:"在桌面应用中运行本地识别；人物资料全 App 共用。"});
}
export async function saveGlobalPerson(displayName:string,person?:GlobalPerson):Promise<GlobalPerson>{
  if(isTauri())return invoke("save_global_person",{input:{id:person?.id??null,displayName,expectedRevision:person?.revision??0,requestId:crypto.randomUUID()}});
  const result={id:person?.id??crypto.randomUUID(),displayName,revision:(person?.revision??0)+1,referenceInstanceIds:person?.referenceInstanceIds??[],tagId:person?.tagId??null};
  const at=demoGlobalPeople.findIndex(p=>p.id===result.id);if(at<0)demoGlobalPeople.push(result);else demoGlobalPeople[at]=result;return structuredClone(result);
}
export async function reviewPersonTuples(workspace:FolderPeopleWorkspace,tuples:PersonTuple[],personId:string,decision:PersonReviewDecision,replaceConfirmed=false):Promise<number>{
  if(!isTauri())throw new Error("请在桌面应用中审阅本地人物实例");
  return invoke("review_person_tuples",{input:{folderPath:workspace.folderPath,workspaceRevision:workspace.revision,tuples,personId,decision,replaceConfirmed,requestId:crypto.randomUUID()}});
}
export async function setGlobalPersonReference(person:GlobalPerson,instanceId:string,enabled:boolean):Promise<void>{
  if(!isTauri())throw new Error("请在桌面应用中设置人物参考");
  return invoke("set_global_person_reference",{input:{personId:person.id,instanceId,enabled,expectedRevision:person.revision,requestId:crypto.randomUUID()}});
}
export async function setGlobalPersonTag(person:GlobalPerson,tagId:number|null):Promise<void>{
  if(!isTauri())throw new Error("请在桌面应用中同步人物标签");
  return invoke("set_global_person_tag",{input:{personId:person.id,tagId,expectedRevision:person.revision,requestId:crypto.randomUUID()}});
}
export async function getGlobalPersonReferences(personId:string):Promise<PersonTuple[]>{return isTauri()?invoke("get_global_person_references",{personId}):[];}
export async function startPeopleGrouping(sessionId:string,folderPath:string,personIds:string[]|null,similarity:number,refreshFeatures=false):Promise<void>{
  if(!isTauri())throw new Error("请在桌面应用中运行人物分组");
  return invoke("start_people_grouping",{sessionId,refreshFeatures,input:{folderPath,personIds,similarity,requestId:crypto.randomUUID()}});
}
export async function getPersonTupleAsset(path:string):Promise<AssetSummary>{
  if(!isTauri()){const asset=demoAssets.find(a=>a.path===path);if(asset)return asset;throw new Error("照片不可用");}
  return invoke("get_person_tuple_asset",{path});
}
export async function savePersonTupleGeometry(folderPath:string,asset:AssetSummary,faceBox:PersonBox|null,bodyBox:PersonBox|null,tuple?:PersonTuple):Promise<PersonTuple>{
  if(!isTauri())throw new Error("请在桌面应用中标记本地人物实例");
  return invoke("save_person_tuple_geometry",{input:{folderPath,assetPath:asset.path,instanceId:tuple?.id??null,expectedRevision:tuple?.revision??0,sourceRevision:`${asset.sizeBytes}:${asset.modifiedAtMs}`,faceBox,bodyBox,requestId:crypto.randomUUID()}});
}
import type { PersonModelStatus, PersonOperationStatus, PersonDetectionSnapshot } from "@/types";

export async function getPersonDetections(folderPath: string, assetPath: string): Promise<PersonDetectionSnapshot> {
  if (!isTauri()) return { sourceRevision: "demo", instances: [] };
  return invoke("get_person_detections", { folderPath, assetPath });
}

export async function adoptPersonDetection(folderPath: string, assetPath: string, sourceRevision: string, instanceId: string): Promise<PersonInstance> {
  if (!isTauri()) throw new Error("请在桌面应用中采用检测结果");
  return invoke("adopt_person_detection", { folderPath, assetPath, sourceRevision, instanceId });
}

export async function getPersonModels(): Promise<PersonModelStatus[]> {
  if (!isTauri()) return [{ id: "demo", name: "人物识别模型", sizeBytes: 0, installed: false, downloadAvailable: false, sourceUrl: "", usage: "请在桌面应用中下载模型并识别本地照片。", error: null }];
  return invoke("get_person_models");
}

export async function getPersonOperation(): Promise<PersonOperationStatus | null> {
  if (!isTauri()) return null;
  return invoke("get_person_operation");
}

export async function downloadPersonModel(modelId: string): Promise<void> {
  if (!isTauri()) throw new Error("请在桌面应用中下载模型");
  await invoke("start_person_model_download", { modelId, requestId: crypto.randomUUID() });
}

export async function importPersonModel(modelId: string): Promise<boolean> {
  if (!isTauri()) throw new Error("请在桌面应用中导入模型");
  const path = await open({ multiple: false, directory: false, filters: [{ name: "ONNX", extensions: ["onnx"] }] });
  if (!path) return false;
  await invoke("import_person_model", { modelId, path, requestId: crypto.randomUUID() });
  return true;
}

export async function startFolderPersonAnalysis(sessionId: string, folderPath: string): Promise<void> {
  if (!isTauri()) throw new Error("请在桌面应用中识别文件夹");
  await invoke("start_folder_person_analysis", { sessionId, folderPath, requestId: crypto.randomUUID() });
}

export async function cancelPersonOperation(operationId: string): Promise<void> {
  if (isTauri()) await invoke("cancel_person_operation", { operationId });
}
import { normalizeCustomTag } from "@/lib/assets/tagTree";
import { retainMediaResource, releaseUnretainedMediaResource } from "@/lib/cache/mediaResourceLease";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  FolderPerson,
  HistoricalPerson,
  PersonTagLink,
  PersonTagOverride,
  PersonInstance,
  PersonReview,
  PersonReviewDecision,
  AboutLink,
  AppInfo,
  ExternalAppSettings,
  ExternalOpenResult,
  RawDecoderStatus,
  UpdateStatus,
  AssetDetails,
  AssetDetailsResult,
  AssetKind,
  AssetQuery,
  AssetSummary,
  BurstGroup,
  CacheSettings,
  DirectorySearchMatch,
  DirectoryBrowseProgress,
  DirectorySummary,
  DirectoryTreeNode,
  DirectoryTreeSnapshot,
  FolderSession,
  HeifCapabilities,
  HeifFullPresentation,
  ImageProjection,
  LibraryIndexUpdate,
  ExiftoolStatus,
  FileDeletionMode,
  MetadataPatch,
  MetadataProjection,
  MetadataRequestPriority,
  Page,
  PerfScenario,
  PreviewPriority,
  PreviewOmittedPolicy,
  PreviewResult,
  PreviewScheduleIntent,
  RenderLevel,
  SchedulePlacement,
  AssetTagAssignment,
  AssetTagAssignmentsByPath,
  CustomTag,
  DebugQueueSnapshot,
  TagDeleteImpact,
  TagSyncStatus,
  WindowDragEvent,
} from "@/types";

const demoPeople = new Map<string, FolderPerson[]>();
const demoPersonInstances = new Map<string, PersonInstance[]>();
const demoPersonReviews = new Map<string, PersonReview>();
const demoHistoricalPeople = new Map<string, HistoricalPerson>();
const demoHistoricalLinks = new Map<string, string>();
const demoPersonTagLinks = new Map<string, PersonTagLink>();
const demoPersonTagOverrides = new Map<string, PersonTagOverride>();
function demoEffectiveTagIds(path: string): Set<number> {
  const ids = new Set(demoAssetTags.get(path) ?? []);
  for (const [subjectId, historyId] of demoHistoricalLinks) {
    const link = demoPersonTagLinks.get(historyId);
    if (!link?.enabled || link.tagId === null || !demoTags.some(tag => tag.id === link.tagId)) continue;
    if (demoPersonTagOverrides.get(`${historyId}:${path}`)?.suppressed) continue;
    if ([...demoPersonReviews.values()].some(review => review.subjectId === subjectId && review.instance.assetPath === path && review.decision === "belongs" && !review.instance.needsReview)) ids.add(link.tagId);
  }
  return ids;
}
let demoPersonId = 0;
const nextDemoPersonId = () => `demo-person-${++demoPersonId}`;

export async function listHistoricalPeople(): Promise<HistoricalPerson[]> {
  if (isTauri()) return invoke("list_historical_people");
  return [...demoHistoricalPeople.values()].sort((a, b) => a.displayName.localeCompare(b.displayName));
}

export async function getHistoricalLink(folderPath: string, subjectId: string): Promise<string | null> {
  if (isTauri()) return invoke("get_historical_link", { folderPath, subjectId });
  if (!(demoPeople.get(folderPath) ?? []).some(person => person.id === subjectId)) return null;
  return demoHistoricalLinks.get(subjectId) ?? null;
}

export async function getPersonTagLink(folderPath: string, subjectId: string): Promise<PersonTagLink | null> {
  if (isTauri()) return invoke("get_person_tag_link", { folderPath, subjectId });
  const historyId = demoHistoricalLinks.get(subjectId);
  return historyId ? demoPersonTagLinks.get(historyId) ?? null : null;
}

export async function setPersonTagLink(person: FolderPerson, tagId: number | null, enabled: boolean, expectedRevision: number): Promise<PersonTagLink> {
  const input = { folderPath: person.folderPath, subjectId: person.id, tagId, enabled, expectedRevision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("set_person_tag_link", { input });
  const historyId = demoHistoricalLinks.get(person.id);
  if (!historyId || (enabled && tagId === null) || (tagId !== null && !demoTags.some(tag => tag.id === tagId))) throw new Error("Historical identity or tag missing");
  const previous = demoPersonTagLinks.get(historyId);
  if ((previous?.revision ?? 0) !== expectedRevision) throw new Error("Person tag link changed");
  const result = { historicalPersonId: historyId, tagId, enabled, revision: expectedRevision + 1 };
  demoPersonTagLinks.set(historyId, result);
  return result;
}

export async function getPersonTagOverride(folderPath: string, subjectId: string, assetPath: string): Promise<PersonTagOverride | null> {
  if (isTauri()) return invoke("get_person_tag_override", { folderPath, subjectId, assetPath });
  const historyId = demoHistoricalLinks.get(subjectId);
  return historyId ? demoPersonTagOverrides.get(`${historyId}:${assetPath}`) ?? null : null;
}

export async function setPersonTagOverride(person: FolderPerson, assetPath: string, suppressed: boolean, expectedRevision: number): Promise<PersonTagOverride> {
  const input = { folderPath: person.folderPath, subjectId: person.id, assetPath, suppressed, expectedRevision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("set_person_tag_override", { input });
  const historyId = demoHistoricalLinks.get(person.id);
  if (!historyId || !(demoAssets.some(item => item.path === assetPath))) throw new Error("Historical identity or asset missing");
  const key = `${historyId}:${assetPath}`;
  if ((demoPersonTagOverrides.get(key)?.revision ?? 0) !== expectedRevision) throw new Error("Person tag override changed");
  const result = { historicalPersonId: historyId, assetPath, suppressed, revision: expectedRevision + 1 };
  demoPersonTagOverrides.set(key, result);
  return result;
}

export async function getFolderPersonReferenceAsset(folderPath: string, subjectId: string): Promise<AssetSummary | null> {
  if (isTauri()) return invoke("get_folder_person_reference_asset", { folderPath, subjectId });
  const person = (demoPeople.get(folderPath) ?? []).find(item => item.id === subjectId);
  const instance = person?.referenceInstanceId ? (demoPersonInstances.get(folderPath) ?? []).find(item => item.id === person.referenceInstanceId) : undefined;
  return demoAssets.find(item => item.path === instance?.assetPath && `${item.sizeBytes}:${item.modifiedAtMs}` === instance.sourceRevision) ?? null;
}

export async function getHistoricalReferenceAsset(historicalPersonId: string): Promise<AssetSummary | null> {
  if (isTauri()) return invoke("get_historical_reference_asset", { historicalPersonId });
  const person = demoHistoricalPeople.get(historicalPersonId);
  return demoAssets.find(item => item.path === person?.referenceAssetPath && `${item.sizeBytes}:${item.modifiedAtMs}` === person.referenceSourceRevision) ?? null;
}

export async function linkHistoricalPerson(person: FolderPerson, historicalPersonId?: string): Promise<HistoricalPerson> {
  const input = { folderPath: person.folderPath, subjectId: person.id, historicalPersonId: historicalPersonId ?? null, expectedRevision: person.revision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("link_historical_person", { input });
  const current = (demoPeople.get(person.folderPath) ?? []).find(item => item.id === person.id);
  const reference = person.referenceInstanceId ? demoPersonReviews.get(`${person.referenceInstanceId}:${person.id}`) : undefined;
  if (!current || current.revision !== person.revision || !current.identityConfirmed || reference?.decision !== "belongs" || !reference.instance.faceBox) throw new Error("Confirmed folder identity and face reference required");
  const previous = demoHistoricalLinks.get(person.id);
  if (previous && (!historicalPersonId || previous === historicalPersonId)) throw new Error("Historical identity already linked");
  const existing = historicalPersonId ? demoHistoricalPeople.get(historicalPersonId) : undefined;
  if (historicalPersonId && !existing) throw new Error("Historical identity not found");
  if (existing && !demoAssets.some(item => item.path === existing.referenceAssetPath && `${item.sizeBytes}:${item.modifiedAtMs}` === existing.referenceSourceRevision)) throw new Error("Historical reference changed");
  const history: HistoricalPerson = existing ?? { id: nextDemoPersonId(), displayName: current.displayName ?? "", referenceAssetPath: reference.instance.assetPath, referenceSourceRevision: reference.instance.sourceRevision, revision: 1 };
  demoHistoricalPeople.set(history.id, history);
  demoHistoricalLinks.set(person.id, history.id);
  demoPeople.set(person.folderPath, (demoPeople.get(person.folderPath) ?? []).map(item => item.id === person.id ? { ...item, revision: item.revision + 1 } : item));
  return history;
}

export async function unlinkHistoricalPerson(person: FolderPerson): Promise<void> {
  const input = { folderPath: person.folderPath, subjectId: person.id, expectedRevision: person.revision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("unlink_historical_person", { input });
  const current = (demoPeople.get(person.folderPath) ?? []).find(item => item.id === person.id);
  if (!current || current.revision !== person.revision || !demoHistoricalLinks.has(person.id)) throw new Error("Historical link changed");
  demoHistoricalLinks.delete(person.id);
  demoPeople.set(person.folderPath, (demoPeople.get(person.folderPath) ?? []).map(item => item.id === person.id ? { ...item, revision: item.revision + 1 } : item));
}

export async function listFolderPeople(folderPath: string): Promise<FolderPerson[]> {
  if (isTauri()) return invoke("list_folder_people", { folderPath });
  return (demoPeople.get(folderPath) ?? []).map(person => ({ ...person, pendingCount: [...demoPersonReviews.values()].filter(review => review.subjectId === person.id && review.decision === "pending").length }));
}

export async function createFolderPerson(folderPath: string): Promise<FolderPerson> {
  const requestId = crypto.randomUUID();
  if (isTauri()) return invoke("create_folder_person", { folderPath, requestId });
  const person: FolderPerson = { id: nextDemoPersonId(), folderPath, displayName: null, identityConfirmed: false, revision: 1 };
  demoPeople.set(folderPath, [...(demoPeople.get(folderPath) ?? []), person]);
  return person;
}

export async function confirmFolderPerson(person: FolderPerson, displayName: string, referenceInstanceId: string): Promise<FolderPerson> {
  const input = { folderPath: person.folderPath, subjectId: person.id, displayName, referenceInstanceId, expectedRevision: person.revision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("confirm_folder_person", { input });
  const reference = demoPersonReviews.get(`${referenceInstanceId}:${person.id}`);
  if (reference?.decision !== "belongs" || !reference.instance.faceBox) throw new Error("Confirm a reviewed face reference first");
  if ((demoPeople.get(person.folderPath) ?? []).find((item) => item.id === person.id)?.revision !== person.revision) throw new Error("Person record changed");
  const updated = { ...person, referenceInstanceId, displayName: displayName.trim(), identityConfirmed: true, revision: person.revision + 1 };
  demoPeople.set(person.folderPath, (demoPeople.get(person.folderPath) ?? []).map((item) => item.id === person.id ? updated : item));
  return updated;
}

export async function listPersonInstances(folderPath: string, assetPath: string): Promise<PersonInstance[]> {
  if (isTauri()) return invoke("list_person_instances", { folderPath, assetPath });
  return (demoPersonInstances.get(folderPath) ?? []).filter((item) => item.assetPath === assetPath);
}

export async function createPersonInstance(
  folderPath: string,
  asset: AssetSummary,
  faceBox: [number, number, number, number] | null,
  bodyBox: [number, number, number, number] | null = null,
): Promise<PersonInstance> {
  const input = { folderPath, assetPath: asset.path, sourceRevision: `${asset.sizeBytes}:${asset.modifiedAtMs}`, faceBox, bodyBox, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("create_person_instance", { input });
  const instance: PersonInstance = { id: nextDemoPersonId(), folderPath, assetPath: asset.path, sourceRevision: input.sourceRevision, faceBox, bodyBox, needsReview: false, revision: 1 };
  demoPersonInstances.set(folderPath, [...(demoPersonInstances.get(folderPath) ?? []), instance]);
  return instance;
}

export async function listPersonReviews(folderPath: string, subjectId: string): Promise<PersonReview[]> {
  if (isTauri()) return invoke("list_person_reviews", { folderPath, subjectId });
  return [...demoPersonReviews.values()].filter((item) => item.instance.folderPath === folderPath && item.subjectId === subjectId);
}

export async function setPersonReview(
  folderPath: string,
  instanceId: string,
  subjectId: string,
  decision: PersonReviewDecision,
  expectedRevision: number,
): Promise<PersonReview> {
  const input = { folderPath, instanceId, subjectId, decision, expectedRevision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("set_person_review", { input });
  const instance = (demoPersonInstances.get(folderPath) ?? []).find((item) => item.id === instanceId);
  if (!instance) throw new Error("Person instance not found");
  if (instance.folderPath !== folderPath || !(demoPeople.get(folderPath) ?? []).some((person) => person.id === subjectId)) throw new Error("Person is outside this folder");
  if ((demoPersonReviews.get(`${instanceId}:${subjectId}`)?.revision ?? 0) !== expectedRevision) throw new Error("Person review changed");
  const review = { instance, subjectId, decision, revision: expectedRevision + 1 };
  demoPersonReviews.set(`${instanceId}:${subjectId}`, review);
  if (decision !== "belongs") demoPeople.set(folderPath, (demoPeople.get(folderPath) ?? []).map(person => person.referenceInstanceId === instanceId && person.id === subjectId ? { ...person, referenceInstanceId: null, revision: person.revision + 1 } : person));
  return review;
}
export async function updatePersonInstance(instance: PersonInstance, asset: AssetSummary, faceBox: PersonInstance["faceBox"], bodyBox: PersonInstance["bodyBox"]): Promise<PersonInstance> {
  const input = { folderPath: instance.folderPath, instanceId: instance.id, sourceRevision: `${asset.sizeBytes}:${asset.modifiedAtMs}`, faceBox, bodyBox, expectedRevision: instance.revision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("update_person_instance", { input });
  const current = (demoPersonInstances.get(instance.folderPath) ?? []).find(item => item.id === instance.id);
  if (!current || current.revision !== instance.revision) throw new Error("Person instance changed");
  const updated = { ...instance, faceBox, bodyBox, sourceRevision: input.sourceRevision, needsReview: false, revision: instance.revision + 1 };
  demoPersonInstances.set(instance.folderPath, (demoPersonInstances.get(instance.folderPath) ?? []).map(item => item.id === instance.id ? updated : item));
  for (const [key, review] of demoPersonReviews) if (review.instance.id === instance.id) demoPersonReviews.set(key, { ...review, instance: updated, decision: review.decision === "doesNotBelong" ? "doesNotBelong" : "pending", revision: review.revision + 1 });
  demoPeople.set(instance.folderPath, (demoPeople.get(instance.folderPath) ?? []).map(person => person.referenceInstanceId === instance.id ? { ...person, referenceInstanceId: null, revision: person.revision + 1 } : person));
  return updated;
}

export async function resetFolderPerson(person: FolderPerson, displayName: string): Promise<void> {
  const input = { folderPath: person.folderPath, subjectId: person.id, displayName, expectedRevision: person.revision, requestId: crypto.randomUUID() };
  if (isTauri()) return invoke("reset_folder_person", { input });
  if ((demoPeople.get(person.folderPath) ?? []).find(item => item.id === person.id)?.revision !== person.revision) throw new Error("Person record changed");
  demoPeople.set(person.folderPath, (demoPeople.get(person.folderPath) ?? []).map(item => item.id === person.id ? { ...item, displayName: displayName.trim(), identityConfirmed: false, referenceInstanceId: null, revision: item.revision + 1 } : item));
  demoHistoricalLinks.delete(person.id);
}

import { getFolderThumbnail, getFolderThumbnailGeneration, preloadFolderThumbnail } from "@/lib/cache/folderThumbnailCache";
import { browserPreloadQueue, orderedPriorityWeight } from "@/lib/preview/previewQueue";
import { acceptImageProjection } from "@/lib/projection/imageProjection";
import { mediaProtocolUrl } from "@/lib/media/mediaProtocolUrl";
import { acceptMetadataProjection } from "@/lib/projection/metadataProjection";
import { perfMark } from "@/lib/diagnostics/perfProbe";
import { recordBrowseTiming } from "@/lib/diagnostics/browseDiagnostics";
import { beginPreviewDebug } from "@/lib/diagnostics/previewDebug";
import { sharedThumbnailRequests } from "@/lib/preview/sharedThumbnailRequests";

function cancelGeneratedPreviewRequest(
  asset: AssetSummary,
  level: RenderLevel,
  requestId: string,
): void {
  void invoke("cancel_preview_request", {
    path: asset.path,
    level,
    requestId,
  }).catch(() => undefined);
}

const demoNames: Array<[string, AssetKind, number]> = [
  ["DSC_4281.NEF", "raw", 42_840_312],
  ["coastline-dawn.CR3", "raw", 51_220_840],
  ["quiet-corner.jpg", "jpeg", 8_340_120],
  ["IMG_7844.HEIC", "heif", 5_892_401],
  ["field-notes.ARW", "raw", 46_270_032],
  ["studio-portrait.jpg", "jpeg", 12_420_119],
  ["night-platform.DNG", "raw", 62_992_811],
  ["water-study.jpg", "jpeg", 9_871_421],
  ["IMG_7912.HEIC", "heif", 6_228_830],
  ["IMG_7920.HIF", "heif", 6_520_440],
  ["market-light.RAF", "raw", 53_009_210],
  ["silver-grain.jpg", "jpeg", 11_084_120],
  ["still-life.ORF", "raw", 24_340_091],
  ["blue-hour.RW2", "raw", 37_771_400],
  ["archive-scan.tiff", "tiff", 88_770_100],
  ["contact-sheet.webp", "webp", 3_120_889],
  ["proof-select.jpg", "jpeg", 10_840_204],
  ["mountain-air.NEF", "raw", 41_128_991],
  ["window-light.jpg", "jpeg", 7_029_402],
];

const demoRoot = "/demo/Field Notes";
const demoDirectories: DirectorySummary[] = [
  { path: `${demoRoot}/Portraits`, name: "Portraits", hasChildren: true },
  { path: `${demoRoot}/Trips`, name: "Trips", hasChildren: true },
  { path: `${demoRoot}/Trips/Coast`, name: "Coast", hasChildren: true },
];
let demoTreeRevision = 0;
const demoDirectoryTrees = new Map<string, DirectoryTreeSnapshot>();
let demoTagId = 3;
let demoTags: CustomTag[] = [
  { id: 1, name: "人物", path: "人物", sortOrder: 0 },
  { id: 2, parentId: 1, name: "家人", path: "人物|家人", sortOrder: 0 },
];
const demoAssetTags = new Map<string, Set<number>>();

function demoTreeChildren(path: string, previous: DirectoryTreeNode[] = []): DirectoryTreeNode[] {
  const previousByPath = new Map(previous.map((node) => [node.entry.path, node]));
  return demoDirectories
    .filter((entry) => entry.path.slice(0, entry.path.lastIndexOf("/")) === path)
    .map((entry) => previousByPath.get(entry.path) ?? {
      entry,
      expanded: false,
      children: null,
    });
}

function findDemoTreeNode(node: DirectoryTreeNode, path: string): DirectoryTreeNode | undefined {
  if (node.entry.path === path) return node;
  return node.children?.map((child) => findDemoTreeNode(child, path)).find(Boolean);
}

function cloneDemoTree(snapshot: DirectoryTreeSnapshot): DirectoryTreeSnapshot {
  const cloneNode = (node: DirectoryTreeNode): DirectoryTreeNode => ({
    entry: { ...node.entry },
    expanded: node.expanded,
    children: node.children?.map(cloneNode) ?? null,
  });
  return { ...snapshot, root: cloneNode(snapshot.root) };
}

function createDemoTree(session: FolderSession): DirectoryTreeSnapshot {
  const snapshot: DirectoryTreeSnapshot = {
    sessionId: session.id,
    revision: ++demoTreeRevision,
    root: {
      entry: { path: session.rootPath, name: session.displayName, hasChildren: true },
      expanded: false,
      children: null,
    },
  };
  demoDirectoryTrees.set(session.id, snapshot);
  return snapshot;
}

const demoAssets: AssetSummary[] = demoNames.map(([name, kind, sizeBytes], index) => ({
  id: `demo-${index}`,
  path: `${index < 10 ? demoRoot : index < 15 ? `${demoRoot}/Portraits` : `${demoRoot}/Trips/Coast`}/${name}`,
  name,
  extension: name.split(".").at(-1)?.toUpperCase() ?? "",
  kind,
  sizeBytes,
  modifiedAtMs: Date.now() - index * 3_600_000,
  hasSidecar: kind === "raw" && index % 3 !== 1,
  rating: index % 6 || undefined,
  colorLabel: ["Red", "Yellow", "Green", "Blue", "Purple"][index % 7],
  pickLabel: index % 5 === 0 ? "accepted" : index % 7 === 0 ? "rejected" : undefined,
}));

export const isTauri = () => "__TAURI_INTERNALS__" in window;

export async function chooseFolder(): Promise<string | null> {
  if (!isTauri()) return "/demo/Field Notes";
  const selection = await open({ directory: true, multiple: false });
  return typeof selection === "string" ? selection : null;
}

// Development fixture: change the query flag before refreshing to simulate a reconnect.
function isUnavailableDemoFolder(path: string): boolean {
  return path === "/demo/Unavailable" && !(__OXY_DEBUG__ && typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).has("folderRecoveryOnline"));
}

export async function openFolder(path: string): Promise<FolderSession> {
  if (!isTauri() && isUnavailableDemoFolder(path)) throw new Error("Folder is unavailable");
  if (!isTauri()) {
    // The browser demo can open more than one folder, so the session must be
    // keyed by path rather than a single shared id.
    const displayName = path.replace(/[\\/]+$/, "").split(/[\\/]/).at(-1) || "Field Notes";
    const session: FolderSession = {
      id: `demo-${path}`,
      rootPath: path,
      displayName,
      openedAtMs: Date.now(),
      deletionMode: "trash",
    };
    createDemoTree(session);
    return session;
  }
  perfMark("folder:open-requested", { path });
  const session = await invoke<FolderSession>("open_folder", { path });
  perfMark("folder:open-returned", { sessionId: session.id });
  return session;
}

export async function listAssets(
  sessionId: string,
  directory: string,
  query: AssetQuery,
  cursor?: number,
  snapshotRevision?: number,
): Promise<Page<AssetSummary>> {
  if (!isTauri()) {
    const needle = query.search?.toLowerCase();
    const tagGroups = (query.tagIds ?? []).map((id) => {
      const root = demoTags.find((tag) => tag.id === id);
      return new Set(root ? demoTags.filter((tag) => tag.id === id || tag.path.startsWith(`${root.path}|`)).map((tag) => tag.id) : []);
    });
    const clusterSnapshot = query.personClusterFilter ? await getPersonClusters(directory) : null;
    const clusterMembers = clusterSnapshot?.snapshotId === query.personClusterFilter?.snapshotId ? (query.personClusterFilter?.clusterId === "ungrouped" ? clusterSnapshot?.ungrouped : clusterSnapshot?.clusters.find(c => c.id === query.personClusterFilter?.clusterId)?.members) : undefined;
    const personReviews = query.personFilter ? await listPersonReviews(directory, query.personFilter.subjectId ?? "") : [];
    const filtered = [...demoAssets]
      .filter(asset => {
        const filter = query.personFilter;
        if (!filter) return true;
        const instances = (demoPersonInstances.get(directory) ?? []).filter(item => item.assetPath === asset.path);
        const valid = (item: PersonInstance) => !item.needsReview && item.sourceRevision === `${asset.sizeBytes}:${asset.modifiedAtMs}`;
        const reviews = personReviews.filter(item => item.instance.assetPath === asset.path && valid(item.instance));
        if (filter.state === "unassigned") return !reviews.length;
        if (filter.state === "needsReview") return instances.some(item => !valid(item));
        return reviews.some(item => filter.state === "all" ? item.decision !== "doesNotBelong" : item.decision === filter.state);
      })
      .filter((asset) => !tagGroups.length || (query.tagMatch === "any"
        ? tagGroups.some((group) => [...demoEffectiveTagIds(asset.path)].some((id) => group.has(id)))
        : tagGroups.every((group) => [...demoEffectiveTagIds(asset.path)].some((id) => group.has(id)))))
      .filter((asset) => !query.personTupleFilter || Boolean(demoGlobalWorkspaces.get(directory)?.groups.find(g=>g.id===query.personTupleFilter?.groupId)?.members.some(t=>t.assetPath===asset.path&&(!query.personTupleFilter?.decision||t.decision===query.personTupleFilter.decision))))
      .filter((asset) => !query.personClusterFilter || Boolean(clusterMembers?.some(m => m.assetPath === asset.path && m.summaryRevision === `${asset.sizeBytes}:${asset.modifiedAtMs}`)))
      .filter((asset) => (!query.personClusterFilter && !query.personFilter && !query.tagIds?.length && needle) || asset.path.slice(0, asset.path.lastIndexOf("/")) === directory)
      .filter((asset) => !query.kind || asset.kind === query.kind)
      .filter((asset) => !query.minimumRating || (asset.rating ?? 0) >= query.minimumRating)
      .filter((asset) => !query.colorLabels?.length ||
        query.colorLabels.some((label) => asset.colorLabel?.toLowerCase() === label.toLowerCase()))
      .filter((asset) => !query.pickLabels?.length ||
        query.pickLabels.some((label) => asset.pickLabel?.toLowerCase() === label.toLowerCase()))
      .filter((asset) => !needle || asset.name.toLowerCase().includes(needle))
      .sort((left, right) => {
        const multiplier = query.direction === "ascending" ? 1 : -1;
        if (query.sort === "size") return (left.sizeBytes - right.sizeBytes) * multiplier;
        if (query.sort === "modified")
          return (left.modifiedAtMs - right.modifiedAtMs) * multiplier;
        return left.name.localeCompare(right.name) * multiplier;
      });
    const start = cursor ?? 0;
    const end = Math.min(start + query.pageSize, filtered.length);
    return {
      items: filtered.slice(start, end),
      nextCursor: end < filtered.length ? end : undefined,
      total: filtered.length,
    };
  }
  const started = performance.now();
  const page = await invoke<Page<AssetSummary>>("list_assets", {
    sessionId,
    directory,
    query,
    cursor,
    snapshotRevision,
  });
  if (!cursor) {
    recordBrowseTiming("first-page-returned", { directory, ipcMs: performance.now() - started, ...page.progress });
    perfMark("assets:first-page-returned", { directory, total: page.total, ipcMs: performance.now() - started, ...page.progress });
  }
  return page;
}

export async function onLibraryIndexUpdated(
  callback: (update: LibraryIndexUpdate) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<LibraryIndexUpdate>("library-index-updated", (event) => callback(event.payload));
}

/**
 * Native drag-and-drop from the OS. Tauri intercepts the platform drop
 * (`dragDropEnabled` defaults to on), so HTML5 drag events never carry a real
 * path; this is the only channel that sees the dropped folders.
 */
export async function onWindowDragDrop(
  callback: (event: WindowDragEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === "enter" || payload.type === "drop") {
      callback({
        type: payload.type,
        paths: payload.paths,
        position: { x: payload.position.x, y: payload.position.y },
      });
      return;
    }
    if (payload.type === "over") {
      callback({ type: "over", position: { x: payload.position.x, y: payload.position.y } });
      return;
    }
    callback({ type: "leave" });
  });
}

export async function onDirectoryBrowseProgress(callback: (progress: DirectoryBrowseProgress) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<DirectoryBrowseProgress>("directory-browse-progress", (event) => callback(event.payload));
}

export async function onLibraryDirectoryIndexUpdated(
  callback: (update: LibraryIndexUpdate) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<LibraryIndexUpdate>("library-directory-index-updated", (event) => callback(event.payload));
}

export async function onDirectoryTreeUpdated(
  callback: (snapshot: DirectoryTreeSnapshot) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<DirectoryTreeSnapshot>("directory-tree-updated", (event) => callback(event.payload));
}

export async function getDirectoryTree(session: FolderSession): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(session.id) ?? createDemoTree(session);
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("get_directory_tree", { sessionId: session.id });
}

export async function setDirectoryExpanded(
  sessionId: string,
  directory: string,
  expanded: boolean,
): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(sessionId);
    if (!snapshot) throw new Error(`Unknown folder session: ${sessionId}`);
    const node = findDemoTreeNode(snapshot.root, directory);
    if (!node) throw new Error(`Unknown directory tree node: ${directory}`);
    const knownLeaf = node.children?.length === 0;
    node.expanded = expanded && !knownLeaf;
    if (node.expanded && node.children === null) {
      node.children = demoTreeChildren(directory);
      node.entry.hasChildren = node.children.length > 0;
      if (node.children.length === 0) node.expanded = false;
    }
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("set_directory_expanded", {
    sessionId,
    directory,
    expanded,
  });
}

export async function collapseDirectoryTree(sessionId: string): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(sessionId);
    if (!snapshot) throw new Error(`Unknown folder session: ${sessionId}`);
    const collapseNode = (node: DirectoryTreeNode) => {
      node.expanded = false;
      node.children?.forEach(collapseNode);
    };
    collapseNode(snapshot.root);
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("collapse_directory_tree", { sessionId });
}

export async function setActiveDirectory(sessionId: string, directory: string): Promise<void> {
  if (!isTauri()) {
    if (!demoDirectoryTrees.has(sessionId)) {
      throw new Error(`Unknown folder session: ${sessionId}`);
    }
    void directory;
    return;
  }
  await invoke("set_active_directory", { sessionId, directory });
}

export async function searchDirectories(
  sessionId: string,
  search: string,
): Promise<DirectorySearchMatch[] | null> {
  if (!isTauri()) {
    const needle = search.trim().toLocaleLowerCase();
    if (!needle) return [];
    return demoDirectories
      .filter((entry) => entry.name.toLocaleLowerCase().includes(needle))
      .map((directory) => {
        const relativeParts = directory.path
          .slice(demoRoot.length)
          .split("/")
          .filter(Boolean);
        let ancestorPath = demoRoot;
        const ancestors = relativeParts.slice(0, -1).map((name) => {
          ancestorPath = `${ancestorPath}/${name}`;
          return { path: ancestorPath, name, hasChildren: true };
        });
        return { directory, ancestors };
      });
  }
  return invoke<DirectorySearchMatch[] | null>("search_directories", { sessionId, search });
}

export async function refreshDirectory(
  sessionId: string,
  directory: string,
): Promise<DirectoryTreeSnapshot> {
  if (!isTauri()) {
    const snapshot = demoDirectoryTrees.get(sessionId);
    if (!snapshot) throw new Error(`Unknown folder session: ${sessionId}`);
    const refreshNode = (node: DirectoryTreeNode) => {
      node.children = demoTreeChildren(node.entry.path, node.children ?? []);
      node.children.forEach(refreshNode);
      node.entry.hasChildren = node.children.length > 0;
      if (node.children.length === 0) node.expanded = false;
    };
    refreshNode(snapshot.root);
    snapshot.revision = ++demoTreeRevision;
    return cloneDemoTree(snapshot);
  }
  return invoke<DirectoryTreeSnapshot>("refresh_directory", { sessionId, directory });
}

export async function deletePaths(paths: string[], mode: FileDeletionMode): Promise<void> {
  if (!isTauri()) {
    for (let index = demoAssets.length - 1; index >= 0; index -= 1) {
      if (paths.some((path) => demoAssets[index].path === path || demoAssets[index].path.startsWith(`${path}/`))) {
        demoAssets.splice(index, 1);
      }
    }
    for (let index = demoDirectories.length - 1; index >= 0; index -= 1) {
      if (paths.some((path) => demoDirectories[index].path === path || demoDirectories[index].path.startsWith(`${path}/`))) {
        demoDirectories.splice(index, 1);
      }
    }
    return;
  }
  await invoke("execute_file_operation", {
    operation: { type: mode === "permanent" ? "deletePermanently" : "trash", paths },
  });
}

export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const input = document.createElement("textarea");
  input.value = text;
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.append(input);
  input.select();
  const copied = document.execCommand("copy");
  input.remove();
  if (!copied) throw new Error("Unable to copy path to the clipboard");
}

export async function openInFileManager(path: string): Promise<void> {
  if (!isTauri()) return;
  await invoke("open_in_file_manager", { path });
}

export async function getExternalAppSettings(): Promise<ExternalAppSettings> {
  if (isTauri()) return invoke("get_external_app_settings");
  const saved = localStorage.getItem("oxyviewer.demo.externalApps");
  return saved ? JSON.parse(saved) : { apps: [], defaultAppId: null };
}

export async function updateExternalAppSettings(settings: ExternalAppSettings): Promise<ExternalAppSettings> {
  if (isTauri()) return invoke("update_external_app_settings", { settings });
  localStorage.setItem("oxyviewer.demo.externalApps", JSON.stringify(settings));
  return settings;
}

export async function chooseExternalApplication(): Promise<string | null> {
  if (!isTauri()) return null;
  const selected = await open({ multiple: false, directory: false, filters: navigator.userAgent.includes("Windows")
    ? [{ name: "Application", extensions: ["exe"] }] : undefined });
  return typeof selected === "string" ? selected : null;
}

export async function openAssetWithApplication(path: string, appId: string): Promise<ExternalOpenResult> {
  if (!isTauri()) throw new Error("Opening external applications requires the desktop app");
  return invoke("open_asset_with_application", { path, appId });
}

export async function openAssetWithSystemDialog(path: string): Promise<ExternalOpenResult> {
  if (!isTauri()) throw new Error("Opening external applications requires the desktop app");
  return invoke("open_asset_with_system_dialog", { path });
}

export async function getAssetDetails(asset: AssetSummary): Promise<AssetDetails> {
  if (!isTauri()) {
    return {
      asset,
      width: 6_240,
      height: 4_160,
      metadata: {
        rating: asset.rating,
        colorLabel: asset.colorLabel,
        pickLabel: asset.pickLabel,
        creator: "OxyViewer Demo",
        copyright: "Personal archive",
        keywords: ["field-notes", asset.kind],
        hierarchicalKeywords: [],
      },
      embeddedKeywords: asset.kind === "heif" ? ["embedded-demo"] : [],
      embeddedHierarchicalKeywords: [],
      metadataCapability: {
        provider: asset.kind === "raw" ? "sidecar" : "native",
        readable: true,
        writable: true,
      },
      sidecarPath: asset.hasSidecar ? asset.path.replace(/\.[^.]+$/, ".xmp") : undefined,
      captureMetadata: {
        aperture: "f/2.8",
        exposureTime: "1/250 s",
        focalLength: "50 mm",
        iso: "100",
        exposureCompensation: "+0.3 EV",
        capturedAt: "2026:08:24 17:42:18",
        cameraMake: "Sony",
        cameraModel: "ILCE-7RM5",
        lensMake: "Sony",
        lensModel: "FE 50mm F1.4 GM",
        chromaSubsampling: asset.kind === "heif" ? "4:2:0" : "4:2:2",
        colorTemperature: "Auto",
        tint: "0",
        dynamicRangeOptimizer: "Auto",
      },
      focusInfo: ["raw", "heif", "jpeg"].includes(asset.kind) ? {
        coordinateWidth: 6_240,
        coordinateHeight: 4_160,
        regions: [{
          centerX: 1_400 + (Number(asset.id.replace("demo-", "")) % 4) * 1_150,
          centerY: 1_350 + (Number(asset.id.replace("demo-", "")) % 3) * 620,
          width: asset.kind === "raw" ? undefined : 240,
          height: asset.kind === "raw" ? undefined : 240,
        }],
      } : undefined,
    };
  }
  const result = await invoke<AssetDetailsResult>("get_asset_details", { path: asset.path });
  acceptMetadataProjection(result.metadataProjection);
  return result.details;
}

export async function requestMetadata(
  paths: string[],
  priority: MetadataRequestPriority,
): Promise<MetadataProjection[]> {
  if (!isTauri()) {
    return demoAssets.filter((asset) => paths.includes(asset.path)).map((asset, index) => ({
      path: asset.path,
      sourceRevision: `${asset.modifiedAtMs}:${asset.sizeBytes}:${asset.hasSidecar ? 1 : 0}`,
      stateRevision: Date.now() + index,
      validAt: Date.now() + index,
      status: "ready",
      rating: asset.rating,
      colorLabel: asset.colorLabel,
      pickLabel: asset.pickLabel,
    }));
  }
  return invoke<MetadataProjection[]>("request_metadata", { paths, priority });
}

/**
 * Groups the given assets into continuous-shooting bursts.
 *
 * Pass the whole current listing in browse order: the scanner parses only the
 * paths it has not seen, so each call costs just the newest page.
 */
export async function scanBurstGroups(paths: string[]): Promise<BurstGroup[]> {
  // The browser demo has no maker notes to read.
  if (!isTauri() || !paths.length) return [];
  return invoke<BurstGroup[]>("scan_burst_groups", { paths });
}

export async function onMetadataProjectionUpdated(
  callback: (projection: MetadataProjection) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<MetadataProjection>("metadata-projection-updated", (event) => callback(event.payload));
}

export async function onImageProjectionUpdated(
  callback: (projection: ImageProjection) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<ImageProjection>("image-projection-updated", (event) => callback(event.payload));
}

export async function patchMetadata(paths: string[], patch: MetadataPatch): Promise<string> {
  if (!isTauri()) {
    // Replace (not mutate) entries so React Query structural sharing sees the
    // change and re-renders, matching the fresh projections Tauri pushes.
    for (let index = 0; index < demoAssets.length; index += 1) {
      const asset = demoAssets[index];
      if (!paths.includes(asset.path)) continue;
      demoAssets[index] = {
        ...asset,
        ...("rating" in patch ? { rating: patch.rating ?? undefined } : {}),
        ...("colorLabel" in patch ? { colorLabel: patch.colorLabel ?? undefined } : {}),
        ...("pickLabel" in patch ? { pickLabel: patch.pickLabel ?? undefined } : {}),
        hasSidecar: true,
      };
    }
    return "demo-metadata-job";
  }
  return invoke<string>("patch_metadata", { paths, patch });
}

function refreshDemoTagPaths(): void {
  const byId = new Map(demoTags.map((tag) => [tag.id, tag]));
  const pathFor = (tag: CustomTag): string => {
    const parent = tag.parentId === undefined ? undefined : byId.get(tag.parentId);
    return parent ? `${pathFor(parent)}|${tag.name}` : tag.name;
  };
  demoTags = demoTags.map((tag) => ({ ...tag, path: pathFor(tag) }));
}

export async function listCustomTags(): Promise<CustomTag[]> {
  if (!isTauri()) return demoTags.map((tag) => ({ ...tag }));
  return (await invoke<CustomTag[]>("list_custom_tags")).map(normalizeCustomTag);
}

export async function getAssetTagAssignments(paths: string[]): Promise<AssetTagAssignment[]> {
  if (!isTauri()) {
    return demoTags.map((tag) => ({
      tag: { ...tag },
      assignedCount: paths.filter((path) => demoEffectiveTagIds(path).has(tag.id)).length,
      assetCount: paths.length,
    }));
  }
  return (await invoke<AssetTagAssignment[]>("get_asset_tag_assignments", { paths }))
    .map((assignment) => ({ ...assignment, tag: normalizeCustomTag(assignment.tag) }));
}

export async function getAssetTagSourceKinds(path: string, tagId: number): Promise<string[]> {
  if (isTauri()) return invoke("get_asset_tag_source_kinds", { path, tagId });
  const kinds: string[] = [];
  if (demoAssetTags.get(path)?.has(tagId)) kinds.push("manual");
  for (const [subjectId, historyId] of demoHistoricalLinks) {
    const link = demoPersonTagLinks.get(historyId);
    if (!link?.enabled || link.tagId !== tagId || demoPersonTagOverrides.get(`${historyId}:${path}`)?.suppressed) continue;
    if ([...demoPersonReviews.values()].some(review => review.subjectId === subjectId && review.instance.assetPath === path && review.decision === "belongs" && !review.instance.needsReview)) {
      kinds.push("person"); break;
    }
  }
  return kinds;
}

export async function getAssetTagAssignmentsByPath(paths: string[]): Promise<AssetTagAssignmentsByPath[]> {
  if (!isTauri()) return Promise.all(paths.map(async (path) => ({
    path,
    assignments: (await getAssetTagAssignments([path])).filter((assignment) => assignment.assignedCount > 0),
  })));
  return (await invoke<AssetTagAssignmentsByPath[]>("get_asset_tag_assignments_by_path", { paths }))
    .map((entry) => ({ ...entry, assignments: entry.assignments.map((assignment) => ({
      ...assignment, tag: normalizeCustomTag(assignment.tag),
    })) }));
}

export async function createCustomTag(parentId: number | undefined, name: string): Promise<CustomTag> {
  if (!isTauri()) {
    const siblings = demoTags.filter((tag) => tag.parentId === parentId);
    const tag: CustomTag = { id: demoTagId++, parentId, name: name.trim(), path: "", sortOrder: siblings.length };
    demoTags.push(tag);
    refreshDemoTagPaths();
    return { ...demoTags.find((item) => item.id === tag.id)! };
  }
  return normalizeCustomTag(await invoke<CustomTag>("create_custom_tag", { parentId: parentId ?? null, name }));
}

export async function updateCustomTag(
  id: number,
  parentId: number | undefined,
  name: string,
): Promise<CustomTag> {
  if (!isTauri()) {
    demoTags = demoTags.map((tag) => tag.id === id ? { ...tag, parentId, name: name.trim() } : tag);
    refreshDemoTagPaths();
    return { ...demoTags.find((tag) => tag.id === id)! };
  }
  return normalizeCustomTag(await invoke<CustomTag>("update_custom_tag", { id, parentId: parentId ?? null, name }));
}

export async function getCustomTagDeleteImpact(id: number): Promise<TagDeleteImpact> {
  if (!isTauri()) {
    const descendants = new Set([id]);
    for (let changed = true; changed;) {
      changed = false;
      for (const tag of demoTags) {
        if (tag.parentId !== undefined && descendants.has(tag.parentId) && !descendants.has(tag.id)) {
          descendants.add(tag.id);
          changed = true;
        }
      }
    }
    const paths = new Set([...demoAssets.map(asset => asset.path), ...demoAssetTags.keys()]);
    const assetCount = [...paths].filter(path => [...descendants].some(tagId => demoEffectiveTagIds(path).has(tagId))).length;
    return { tagCount: descendants.size, assetCount };
  }
  return invoke<TagDeleteImpact>("get_custom_tag_delete_impact", { id });
}

export async function deleteCustomTag(id: number): Promise<TagDeleteImpact> {
  if (!isTauri()) {
    const impact = await getCustomTagDeleteImpact(id);
    const remove = new Set<number>();
    const collect = (tagId: number) => {
      remove.add(tagId);
      demoTags.filter((tag) => tag.parentId === tagId).forEach((tag) => collect(tag.id));
    };
    collect(id);
    demoTags = demoTags.filter((tag) => !remove.has(tag.id));
    for (const ids of demoAssetTags.values()) for (const tagId of remove) ids.delete(tagId);
    for (const [historyId, link] of demoPersonTagLinks) if (link.tagId !== null && remove.has(link.tagId)) demoPersonTagLinks.set(historyId, { ...link, tagId: null, enabled: false });
    return impact;
  }
  return invoke<TagDeleteImpact>("delete_custom_tag", { id });
}

export async function setAssetCustomTag(paths: string[], tagId: number, assigned: boolean): Promise<void> {
  if (!isTauri()) {
    for (const path of paths) {
      const ids = demoAssetTags.get(path) ?? new Set<number>();
      if (assigned) ids.add(tagId); else ids.delete(tagId);
      demoAssetTags.set(path, ids);
    }
    return;
  }
  await invoke("set_asset_custom_tag", { paths, tagId, assigned });
}

export async function getTagSyncStatus(): Promise<TagSyncStatus> {
  if (!isTauri()) return { pendingCount: 0, failedCount: 0 };
  return invoke<TagSyncStatus>("get_tag_sync_status");
}

export async function retryTagXmpSync(): Promise<void> {
  if (!isTauri()) return;
  await invoke("retry_tag_xmp_sync");
}

export async function syncMetadataToEmbedded(paths: string[]): Promise<string> {
  if (!isTauri()) return "demo-embedded-sync-job";
  return invoke<string>("sync_metadata_to_embedded", { paths });
}

export async function getExiftoolStatus(): Promise<ExiftoolStatus> {
  if (!isTauri()) return { available: true, source: "path", version: "demo" };
  return invoke<ExiftoolStatus>("get_exiftool_status");
}

export async function installExiftool(): Promise<ExiftoolStatus> {
  if (!isTauri()) return { available: true, source: "managed", version: "demo" };
  return invoke<ExiftoolStatus>("install_exiftool");
}

export async function chooseAndConfigureExiftool(): Promise<ExiftoolStatus | null> {
  if (!isTauri()) return { available: true, source: "user", version: "demo" };
  const selection = await open({
    directory: false,
    multiple: false,
    title: "Select ExifTool executable",
  });
  if (typeof selection !== "string") return null;
  return invoke<ExiftoolStatus>("configure_exiftool", { path: selection });
}

const demoRoots: string[] = __OXY_DEBUG__ && typeof window !== "undefined" && new URLSearchParams(window.location.search).has("folderRecoveryDemo") ? ["/demo/Unavailable"] : [];

export async function addLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) {
    if (!demoRoots.includes(path)) demoRoots.push(path);
    return [...demoRoots];
  }
  return invoke<string[]>("add_library_root", { path });
}

export async function removeLibraryRoot(path: string): Promise<string[]> {
  if (!isTauri()) {
    const index = demoRoots.indexOf(path);
    if (index >= 0) demoRoots.splice(index, 1);
    return [...demoRoots];
  }
  return invoke<string[]>("remove_library_root", { path });
}

export async function listLibraryRoots(): Promise<string[]> {
  if (!isTauri()) return [...demoRoots];
  return invoke<string[]>("list_library_roots");
}

export async function reorderLibraryRoots(paths: string[]): Promise<string[]> {
  if (!isTauri()) {
    if (paths.length !== demoRoots.length || paths.some((path) => !demoRoots.includes(path))) {
      throw new Error("Folder order must contain every library root exactly once");
    }
    demoRoots.splice(0, demoRoots.length, ...paths);
    return [...demoRoots];
  }
  return invoke<string[]>("reorder_library_roots", { paths });
}

/**
 * Metadata mirrored from the desktop bundle for the browser demo, which has no
 * backend to report it. `__OXY_APP_VERSION__` comes from apps/desktop/package.json,
 * so the demo version cannot drift from the shipped one.
 */
const demoAppInfo: AppInfo = {
  name: "OxyViewer",
  version: __OXY_APP_VERSION__,
  repositoryUrl: "https://github.com/oxygenkun/oxyviewer",
  author: "oxygenkun",
  license: "AGPL-3.0-only OR LicenseRef-OxyViewer-Commercial",
};

let demoCacheSettings: CacheSettings = {
  location: "/demo/OxyViewer Cache/previews",
  defaultLocation: "/demo/OxyViewer Cache/previews",
  customParent: undefined,
  isCustomLocation: false,
  maxSizeBytes: 10 * 1024 ** 3,
  usedSizeBytes: 2.4 * 1024 ** 3,
};

export async function getCacheSettings(): Promise<CacheSettings> {
  if (!isTauri()) return demoCacheSettings;
  return invoke<CacheSettings>("get_cache_settings");
}

export async function chooseCacheParent(): Promise<string | null> {
  if (!isTauri()) return "/demo/Fast SSD";
  const selection = await open({
    directory: true,
    multiple: false,
    title: "Choose cache location",
  });
  return typeof selection === "string" ? selection : null;
}

export async function updateCacheSettings(
  customParent: string | null,
  maxSizeBytes: number,
): Promise<CacheSettings> {
  if (!isTauri()) {
    demoCacheSettings = {
      ...demoCacheSettings,
      location: customParent
        ? `${customParent}/OxyViewer Cache/previews`
        : demoCacheSettings.defaultLocation,
      customParent: customParent ?? undefined,
      isCustomLocation: Boolean(customParent),
      maxSizeBytes,
      usedSizeBytes: Math.min(demoCacheSettings.usedSizeBytes, maxSizeBytes),
    };
    return demoCacheSettings;
  }
  return invoke<CacheSettings>("update_cache_settings", {
    update: { customParent, maxSizeBytes },
  });
}

export async function clearPreviewCache(): Promise<CacheSettings> {
  if (!isTauri()) {
    demoCacheSettings = { ...demoCacheSettings, usedSizeBytes: 0 };
    return demoCacheSettings;
  }
  return invoke<CacheSettings>("clear_preview_cache");
}

export async function getDebugQueueSnapshot(): Promise<DebugQueueSnapshot> {
  if (!__OXY_DEBUG__) throw new Error("Queue diagnostics require a debug build");
  if (!isTauri()) {
    return { capturedAtUnixMs: Date.now(), workerWaitMicros: 0, collectionMicros: 0, staleQueues: [], queues: [] };
  }
  return invoke<DebugQueueSnapshot>("get_debug_queue_snapshot");
}

export async function openDebugQueueWindow(): Promise<void> {
  if (!__OXY_DEBUG__) return;
  if (!isTauri()) {
    window.open("?debug=queues", "oxyviewer-debug-queues", "popup,width=1180,height=760");
    return;
  }
  await invoke("open_debug_queue_window");
}

export async function closeDebugQueueWindow(): Promise<void> {
  if (!isTauri()) {
    window.close();
    return;
  }
  await invoke("close_debug_queue_window");
}

export function previewUrl(asset: AssetSummary): string | undefined {
  // Native raster files use the same registered resource boundary as every
  // other format. Browser demo assets remain ordinary browser URLs.
  return isTauri() ? undefined : asset.path;
}

export async function generatedPreview(
  asset: AssetSummary,
  level: RenderLevel,
  signal?: AbortSignal,
  priority: PreviewPriority = "visible",
  rank = 0,
  crossFolder = false,
): Promise<PreviewResult | undefined> {
  if (!isTauri()) return undefined;
  if (crossFolder) return requestGeneratedPreview(asset, level, signal, priority, rank, true);
  if (level === "thumbnail") {
    return sharedThumbnailRequests.request(asset, signal, priority, rank, (sharedSignal, sharedPriority, sharedRank) =>
      requestGeneratedPreview(asset, level, sharedSignal, sharedPriority, sharedRank));
  }
  return requestGeneratedPreview(asset, level, signal, priority, rank);
}

async function requestGeneratedPreview(
  asset: AssetSummary,
  level: RenderLevel,
  signal: AbortSignal | undefined,
  priority: PreviewPriority,
  rank: number,
  crossFolder = false,
): Promise<PreviewResult | undefined> {
  // Do not register a debug WAIT entry for work React Query has already
  // cancelled. There is no lifecycle handle to clean up if we throw first.
  if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
  const debug = __OXY_DEBUG__
      ? beginPreviewDebug({
          assetName: asset.name,
          stage: level,
          priority,
          resourceKey: `preview:${asset.path}:${level}`,
          resourceLabel: level,
        })
    : undefined;
  debug?.start();
  perfMark("preview:queued", { assetName: asset.name, level, priority });
  const requestId = crypto.randomUUID();
  const backendRequest = invoke<ImageProjection>("get_preview", {
    requestId,
    path: asset.path,
    level,
    priority,
    rank,
    crossFolder,
  }).then((projection) => {
    if (signal?.aborted && projection.result?.resource) {
      releaseUnretainedMediaResource(projection.result.resource.resourceId);
    }
    return projection;
  });
  let rejectAbort: ((reason: unknown) => void) | undefined;
  const aborted = new Promise<never>((_resolve, reject) => {
    rejectAbort = reject;
  });
  const stopWaiting = () => {
    cancelGeneratedPreviewRequest(asset, level, requestId);
    rejectAbort?.(signal?.reason ?? new DOMException("Aborted", "AbortError"));
  };
  signal?.addEventListener("abort", stopWaiting, { once: true });
  let interim = false;
  try {
    const projection = await (signal ? Promise.race([backendRequest, aborted]) : backendRequest);
    if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
    if (!acceptImageProjection(projection)) {
      debug?.cancel();
      return undefined;
    }
    const result = projection.result;
    if (!result) throw new Error("Ready image projection has no artifact");
    perfMark("preview:result", {
      assetName: asset.name,
      level,
      priority,
      width: result.width,
      height: result.height,
      kind: result.kind,
      renderLevel: result.renderLevel,
      diagnostics: result.diagnostics,
    });
    debug?.mark("backend-result", {
      width: result.width,
      height: result.height,
      kind: result.kind,
      renderLevel: result.renderLevel,
      diagnostics: result.diagnostics,
    });
    debug?.complete({ diagnostics: result.diagnostics });
    if (!result.resource) throw new Error("Ready image projection has no registered resource");
    // Interim is a displayable query result, not a promise to hide behind.
    // Rust keeps the native request alive through its upgrade and publishes
    // either Satisfied or terminal Error into the projection store. Settling
    // here also lets independent full-detail renderers (HEIF tiles) start.
    interim = level !== "thumbnail" && result.satisfaction === "interim";
    return { ...result, url: mediaProtocolUrl(result.resource.url) };
  } catch (error) {
    if (signal?.aborted) debug?.cancel();
    else debug?.fail(error);
    throw error;
  } finally {
    if (!interim) signal?.removeEventListener("abort", stopWaiting);
  }
}

export async function renewMediaResource(resourceId: string): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("renew_media_resource", { resourceId });
}

export async function releaseMediaResource(resourceId: string): Promise<void> {
  if (!isTauri()) return;
  await invoke("release_media_resource", { resourceId });
}

export async function reconcilePreviewSchedule(
  scopeId: string,
  epoch: number,
  intents: PreviewScheduleIntent[],
  omittedPolicy: PreviewOmittedPolicy,
): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("reconcile_preview_schedule", {
    scopeId,
    epoch,
    intents,
    omittedPolicy,
  });
}

export async function upsertPreviewSchedule(
  scopeId: string,
  epoch: number,
  intent: Omit<PreviewScheduleIntent, "rank">,
  placement: SchedulePlacement,
): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("upsert_preview_schedule", {
    scopeId,
    epoch,
    path: intent.path,
    level: intent.level,
    priority: intent.priority,
    placement,
  });
}

export async function releasePreviewSchedule(
  scopeId: string,
  epoch: number,
  path: string,
  level: RenderLevel,
): Promise<boolean> {
  if (!isTauri()) return true;
  return invoke<boolean>("release_preview_schedule", { scopeId, epoch, path, level });
}

/**
 * Requests real thumbnail work and retains a small, native-resource-independent
 * browser copy for the entire current directory. Nearby callers can jump ahead.
 */
export async function preloadAssetThumbnail(
  asset: AssetSummary,
  signal?: AbortSignal,
  priority: PreviewPriority = "preload",
  rank = 0,
): Promise<void> {
  signal?.throwIfAborted();
  if (getFolderThumbnail(asset)) return;
  const generation = getFolderThumbnailGeneration();
  const directSource = previewUrl(asset);
  if (directSource) {
    await browserPreloadQueue.enqueue(
      orderedPriorityWeight(priority, rank),
      signal,
      () => preloadFolderThumbnail(asset, directSource, undefined, signal, generation),
    );
    return;
  }
  const result = await generatedPreview(asset, "thumbnail", signal, priority, rank);
  if (result) {
    const id = result.resource?.resourceId;
    const release = id ? retainMediaResource(id) : undefined;
    try {
      await browserPreloadQueue.enqueue(
        orderedPriorityWeight(priority, rank),
        signal,
        async () => {
          signal?.throwIfAborted();
          if (generation !== getFolderThumbnailGeneration()) return;
          if (id && !await renewMediaResource(id)) throw new Error("Thumbnail resource expired before preload");
          await preloadFolderThumbnail(asset, result.url, result.geometry, signal, generation);
        },
      );
    } finally { release?.(); }
  }
}

export async function startHeifFull(
  path: string,
  generation: number,
  displaySharpening: boolean,
  signal?: AbortSignal,
): Promise<
  | { delivery: "artifact"; projection: ImageProjection; result: PreviewResult }
  | Extract<HeifFullPresentation, { delivery: "tiles" }>
> {
  signal?.throwIfAborted();
  perfMark("heif:decode-requested", { path });
  const requestId = crypto.randomUUID();
  const cancel = () => {
    void invoke("cancel_preview_request", { path, level: "full", requestId }).catch(() => {});
  };
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    const presentation = await invoke<HeifFullPresentation>("start_heif_full", {
      requestId, path, generation, displaySharpening,
    });
    if (signal?.aborted) {
      if (presentation.delivery === "artifact") {
        const id = presentation.projection.result?.resource?.resourceId;
        if (id) releaseUnretainedMediaResource(id);
      } else {
        await cancelHeifDecode(presentation.session.id);
      }
      signal.throwIfAborted();
    }
    if (presentation.delivery === "artifact") {
      const result = presentation.projection.result;
      if (!result) throw new Error("ready HEIF full projection has no artifact");
      if (!result.resource) throw new Error("Ready HEIF artifact has no registered resource");
      return {
        ...presentation,
        result: { ...result, url: mediaProtocolUrl(result.resource.url) },
      };
    }
    perfMark("heif:decode-session", { path, sessionId: presentation.session.id });
    return presentation;
  } finally {
    signal?.removeEventListener("abort", cancel);
  }
}

export async function cancelHeifDecode(sessionId: string): Promise<boolean> {
  return invoke<boolean>("cancel_heif_decode", { sessionId });
}

export async function getHeifCapabilities(): Promise<HeifCapabilities[]> {
  if (!isTauri()) return [];
  return invoke<HeifCapabilities[]>("get_heif_capabilities");
}

export async function getRawDecoderStatus(path?: string, refresh = false): Promise<RawDecoderStatus> {
  if (!isTauri()) return { installAvailable: false, availability: "unsupportedPlatform", codecs: [], detail: null, attempt: null };
  return invoke<RawDecoderStatus>("get_raw_decoder_status", { path: path ?? null, refresh });
}

export async function retryRawFull(path: string): Promise<void> {
  if (isTauri()) await invoke("retry_raw_full", { path });
}

export async function openRawDecoderInstallPage(web = false): Promise<"store" | "web"> {
  if (!isTauri()) {
    window.open("https://apps.microsoft.com/detail/9nctdw2w1bh8", "_blank", "noopener,noreferrer");
    return "web";
  }
  return invoke("open_raw_decoder_install_page", { web });
}

export function heifTileUrl(url: string): string {
  return mediaProtocolUrl(url);
}

/** Returns the E2E performance scenario injected by the runner, if any. */
export async function getPerfScenario(): Promise<PerfScenario | undefined> {
  if (!isTauri()) return undefined;
  return (await invoke<PerfScenario | null>("get_perf_scenario")) ?? undefined;
}

/** Persists the performance report JSON via the backend. */
export async function writePerfReport(path: string, report: unknown): Promise<void> {
  if (!isTauri()) {
    console.info("[OxyPerf] report", report);
    return;
  }
  await invoke("write_perf_report", { path, contents: JSON.stringify(report, null, 2) });
}

/** Explicit native resource diagnostics for debug/performance harnesses. */
export async function getMediaResourceStats(): Promise<import("@/types").ResourceRegistryStats | undefined> {
  if (!isTauri()) return undefined;
  return invoke("get_media_resource_stats");
}

/**
 * Build metadata for the About panel.
 *
 * The desktop build reads the version from the running package. The browser demo
 * has no backend, so it reports the version injected at build time.
 */
export async function getAppInfo(): Promise<AppInfo> {
  if (!isTauri()) return demoAppInfo;
  return invoke<AppInfo>("get_app_info");
}

/**
 * Queries GitHub Releases for the newest published release.
 *
 * Only user-initiated: the app never polls for updates in the background, and it
 * never downloads or installs one from this path.
 */
export async function checkForUpdates(): Promise<UpdateStatus> {
  if (!isTauri()) {
    await new Promise((resolve) => window.setTimeout(resolve, 400));
    return {
      currentVersion: demoAppInfo.version,
      latestVersion: demoAppInfo.version,
      updateAvailable: false,
      releaseUrl: `${demoAppInfo.repositoryUrl}/releases`,
    };
  }
  return invoke<UpdateStatus>("check_for_updates");
}

/**
 * Opens one of the About panel's fixed destinations in the system browser.
 * `releaseUrl` is accepted only when it points at an OxyViewer release page.
 */
export async function openAboutLink(target: AboutLink, releaseUrl?: string): Promise<void> {
  if (!isTauri()) {
    const destinations: Record<AboutLink, string> = {
      repository: demoAppInfo.repositoryUrl,
      releases: `${demoAppInfo.repositoryUrl}/releases`,
      license: `${demoAppInfo.repositoryUrl}/blob/main/LICENSE.md`,
    };
    window.open(destinations[target], "_blank", "noopener,noreferrer");
    return;
  }
  await invoke("open_about_link", { target, releaseUrl: releaseUrl ?? null });
}

export async function checkLibraryRoot(path: string): Promise<void> {
  if (!isTauri()) { if (isUnavailableDemoFolder(path)) throw new Error("Folder is unavailable"); return; }
  return invoke("check_library_root", { path });
}
export async function planRootRelocation(oldRoot: string, newRoot: string): Promise<RootRelocationPlan> {
  if (isTauri()) return invoke("plan_root_relocation", { oldRoot, newRoot });
  if (oldRoot === newRoot || demoRoots.includes(newRoot)) throw new Error("Destination is already registered");
  return { oldRoot, newRoot, entries: ["verified", "unverified", "missing"].map((status, index) => ({
    oldPath: `${oldRoot}/${index + 1}.jpg`, newPath: `${newRoot}/${index + 1}.jpg`,
    status: status as "verified" | "unverified" | "missing", oldIdentityRevision: null, newIdentityRevision: null,
  })) };
}
export async function relocateLibraryRoot(plan: RootRelocationPlan): Promise<RootRelocationResult> {
  if (isTauri()) return invoke("relocate_library_root", { plan });
  const index = demoRoots.indexOf(plan.oldRoot);
  if (index < 0 || demoRoots.includes(plan.newRoot)) throw new Error("Folder list changed");
  demoRoots[index] = plan.newRoot;
  return { rootPath: plan.newRoot, linkedFiles: 2, needsReview: 2, missingFiles: 1, reusedArtifacts: 1, cacheFailures: 0 };
}

export async function getPersonClusters(folderPath: string): Promise<import("@/types").PersonClusterSnapshot | null> {
  if (!isTauri()) return null;
  return invoke("get_person_clusters", { folderPath });
}
export async function startPersonClustering(sessionId: string, folderPath: string): Promise<void> {
  if (!isTauri()) throw new Error("请在桌面应用中聚类本地照片");
  await invoke("start_person_clustering", { sessionId, folderPath, requestId: crypto.randomUUID() });
}
export async function adoptPersonCluster(folderPath: string, snapshotId: string, clusterId: string, subjectId?: string, displayName?: string): Promise<import("@/types").AdoptPersonClusterResult> {
  if (!isTauri()) throw new Error("请在桌面应用中采用分组");
  return invoke("adopt_person_cluster", { input: { folderPath, snapshotId, clusterId, subjectId, displayName, requestId: crypto.randomUUID() } });
}
