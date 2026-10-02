// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { AutomaticPersonBoxes } from "./AutomaticPersonBoxes";
import { PersonDetectionContext } from "./PeopleContext";
import type { AssetSummary } from "@/types";
vi.mock("@/lib/api", () => ({ getPersonDetections: vi.fn(), listPersonInstances: vi.fn() }));
it("shows the selected cluster face even after adoption when no manual-person overlay is active", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const asset: AssetSummary = { id:"a",path:"C:/folder/a.jpg",name:"a.jpg",extension:"jpg",kind:"jpeg",sizeBytes:1,modifiedAtMs:1,hasSidecar:false };
  const faceBox=[0.1,0.1,0.2,0.2];
  const client=new QueryClient({ defaultOptions: { queries: { staleTime: Infinity } } });
  client.setQueryData(["person-detections","C:/folder",asset.path,1,1],{ sourceRevision:"v",instances:[{instanceId:"target",faceBox},{instanceId:"other",faceBox:[0.5,0.1,0.2,0.2]}] });
  client.setQueryData(["person-instances","C:/folder",asset.path],[{id:"manual-target",faceBox}]);
  const host=document.createElement("div"); const root=createRoot(host);
  try {
    await act(async () => root.render(<QueryClientProvider client={client}><PersonDetectionContext.Provider value={{folderPath:"C:/folder",showBoxes:true,instanceIds:["target"],adopt:vi.fn()}}><AutomaticPersonBoxes asset={asset}/></PersonDetectionContext.Provider></QueryClientProvider>));
    expect(host.querySelectorAll(".person-image-box")).toHaveLength(1);
    expect(host.textContent).toContain("本组人脸");
  } finally { await act(async () => root.unmount()); client.clear(); vi.unstubAllGlobals(); }
});
