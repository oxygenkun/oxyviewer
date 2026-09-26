// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { confirmFolderPerson, createCustomTag, createFolderPerson, createPersonInstance, deleteCustomTag, getAssetTagAssignments, getAssetTagSourceKinds, getHistoricalLink, getPersonTagLink, linkHistoricalPerson, listAssets, listFolderPeople, listHistoricalPeople, listPersonReviews, resetFolderPerson, setAssetCustomTag, setPersonReview, setPersonTagLink, setPersonTagOverride, unlinkHistoricalPerson, updatePersonInstance } from "./api";
import type { AssetQuery } from "@/types";

describe("manual people workflow demo contract", () => {
  it("projects an explicitly linked historical person tag and preserves a manual source", async () => {
    const folder = "/demo/Field Notes/Portraits";
    const image = (await listAssets("demo", folder, {sort:"name",direction:"ascending",pageSize:1})).items[0];
    const tag = await createCustomTag(undefined, "Demo person tag");
    const person = await createFolderPerson(folder);
    const instance = await createPersonInstance(folder,image,[.1,.1,.2,.2]);
    await setPersonReview(folder,instance.id,person.id,"belongs",0);
    const confirmed = await confirmFolderPerson(person,"Alex",instance.id);
    await linkHistoricalPerson(confirmed);
    const enabled = await setPersonTagLink(confirmed,tag.id,true,0);
    expect((await getAssetTagAssignments([image.path])).find(item => item.tag.id === tag.id)?.assignedCount).toBe(1);
    expect(await getAssetTagSourceKinds(image.path,tag.id)).toEqual(["person"]);
    const suppressed = await setPersonTagOverride(confirmed,image.path,true,0);
    expect((await getAssetTagAssignments([image.path])).find(item => item.tag.id === tag.id)?.assignedCount).toBe(0);
    await setAssetCustomTag([image.path],tag.id,true);
    expect(await getAssetTagSourceKinds(image.path,tag.id)).toEqual(["manual"]);
    await setPersonTagOverride(confirmed,image.path,false,suppressed.revision);
    expect(await getAssetTagSourceKinds(image.path,tag.id)).toEqual(["manual","person"]);
    await setAssetCustomTag([image.path],tag.id,false);
    expect((await getAssetTagAssignments([image.path])).find(item => item.tag.id === tag.id)?.assignedCount).toBe(1);
    await setPersonTagLink(confirmed,tag.id,false,enabled.revision);
    expect((await getAssetTagAssignments([image.path])).find(item => item.tag.id === tag.id)?.assignedCount).toBe(0);
    await deleteCustomTag(tag.id);
    expect((await getPersonTagLink(folder,person.id))?.tagId).toBeNull();
  });
  it("links confirmed folder identities to one historical person and clears the link on reset", async () => {
    const folders = ["/demo/Field Notes", "/demo/Field Notes/Portraits"];
    const confirmed = [];
    for (const folder of folders) {
      const scopedAsset = (await listAssets("demo", folder, {sort:"name",direction:"ascending",pageSize:1})).items[0];
      const person = await createFolderPerson(folder);
      const instance = await createPersonInstance(folder, scopedAsset, [.1,.1,.2,.2]);
      await setPersonReview(folder,instance.id,person.id,"belongs",0);
      confirmed.push(await confirmFolderPerson(person,"Alex",instance.id));
    }
    const history = await linkHistoricalPerson(confirmed[0]);
    expect(await getHistoricalLink(folders[0],confirmed[0].id)).toBe(history.id);
    await linkHistoricalPerson(confirmed[1],history.id);
    expect(await getHistoricalLink(folders[1],confirmed[1].id)).toBe(history.id);
    expect((await listHistoricalPeople()).filter(item => item.id === history.id)).toHaveLength(1);
    const current = (await listFolderPeople(folders[0]))[0];
    await unlinkHistoricalPerson(current);
    expect(await getHistoricalLink(folders[0],current.id)).toBeNull();
    expect(await getHistoricalLink(folders[1],confirmed[1].id)).toBe(history.id);
    const unlinked = (await listFolderPeople(folders[0]))[0];
    await resetFolderPerson(unlinked,"Alex");
    expect((await listFolderPeople(folders[0]))[0].identityConfirmed).toBe(false);
  });
  it("pages distinct photos, keeps another face pending, excludes negatives and clears invalid references", async () => {
    const folder = "/demo/Field Notes";
    const query: AssetQuery = {sort:"name",direction:"ascending",pageSize:1};
    const all = await listAssets("demo",folder,{...query,pageSize:250});
    const person = await createFolderPerson(folder);
    const first = await createPersonInstance(folder,all.items[0],[.1,.1,.2,.2]);
    const second = await createPersonInstance(folder,all.items[0],[.6,.1,.2,.2]);
    const last = await createPersonInstance(folder,all.items.at(-1)!,[.1,.1,.2,.2]);
    for(const instance of [first,second,last]) await setPersonReview(folder,instance.id,person.id,"pending",0);
    const filter = {...query,personFilter:{subjectId:person.id,state:"pending" as const}};
    const page = await listAssets("demo",folder,filter);
    expect(page.total).toBe(2);
    expect((await listAssets("demo",folder,filter,page.nextCursor)).items[0].path).toBe(last.assetPath);
    await setPersonReview(folder,first.id,person.id,"belongs",1);
    expect((await listAssets("demo",folder,filter)).total).toBe(2);
    await confirmFolderPerson(person,"Alex",first.id);
    await setPersonReview(folder,first.id,person.id,"doesNotBelong",2);
    expect((await listFolderPeople(folder)).find(item=>item.id===person.id)?.referenceInstanceId).toBeNull();
    await updatePersonInstance(first,all.items[0],[.2,.2,.2,.2],null);
    expect((await listPersonReviews(folder,person.id)).find(item=>item.instance.id===first.id)?.decision).toBe("doesNotBelong");
    await setPersonReview(folder,second.id,person.id,"deferred",1);
    expect((await listAssets("demo",folder,filter)).items[0].path).toBe(last.assetPath);
    const saved = (await listFolderPeople(folder)).find(item=>item.id===person.id)!;
    await resetFolderPerson(saved,"Temporary");
    expect((await listPersonReviews(folder,person.id)).length).toBe(3);
    expect((await listFolderPeople(folder)).find(item=>item.id===person.id)?.identityConfirmed).toBe(false);
    expect((await listAssets("demo",folder,{...filter,search:"not-present"})).total).toBe(0);
  });
});
