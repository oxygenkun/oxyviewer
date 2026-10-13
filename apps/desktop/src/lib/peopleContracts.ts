export type { GlobalPerson, GlobalPersonGallery, GlobalPersonFolder, GlobalPersonFolders, PersonTuple, PersonTupleGroup, FolderPeopleWorkspace } from '../generated/contracts';
export type { PersonTupleFilter } from "@/types";
export type PersonBox = NonNullable<import('../generated/contracts').PersonTuple['faceBox']>;
