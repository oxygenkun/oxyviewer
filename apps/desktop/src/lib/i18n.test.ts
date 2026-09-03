import { describe, expect, it } from "vitest";
import { translate } from "./i18n";

describe("translate", () => {
  it("resolves both supported locales", () => {
    expect(translate("zh-CN", "openFolder")).toBe("打开文件夹");
    expect(translate("en", "openFolder")).toBe("Open folder");
  });

  it("localizes view mode labels", () => {
    expect(translate("zh-CN", "viewGrid")).toBe("网格");
    expect(translate("zh-CN", "viewList")).toBe("列表");
    expect(translate("zh-CN", "viewLoupe")).toBe("放大查看");
  });
});

