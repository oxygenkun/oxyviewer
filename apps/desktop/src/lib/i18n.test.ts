import { describe, expect, it } from "vitest";
import { translate } from "./i18n";

describe("translate", () => {
  it("resolves both supported locales", () => {
    expect(translate("zh-CN", "openFolder")).toBe("打开文件夹");
    expect(translate("en", "openFolder")).toBe("Open folder");
  });
});

