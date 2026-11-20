import { render } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import Markdown, { softBreaks } from "./Markdown";

describe("嵌套列表", () => {
  const md = [
    "## 流程",
    "",
    "1. **确定 PR**：用户给的链接。",
    "2. **拉取 PR 元数据和 diff**",
    "   - `pr_fetch.py --url <PR链接>`",
    "   - 标准输出是 JSON。",
    "   - 退出码 3：私有仓库没配 token。",
    "3. **同步代码到 PR 最新提交**",
    "   - `sync_code.py <project> --pr <number>`",
    "",
    "4. **读上一轮结果**",
  ].join("\n");

  test("缩进的 - 子项挂在上一个编号项下面，外层编号 1–4 连续", () => {
    const { container } = render(<Markdown text={md} />);
    const top = container.querySelector(".md-view > ol")!;
    const topItems = Array.from(top.children);
    expect(topItems).toHaveLength(4); // 子项不再占用外层编号，空行也没把列表断开
    expect(topItems[0].textContent).toContain("确定 PR");
    expect(topItems[3].textContent).toContain("读上一轮结果");
    const sub = topItems[1].querySelector(":scope > ul")!;
    expect(sub.children).toHaveLength(3);
    expect(topItems[2].querySelector(":scope > ul")!.children).toHaveLength(1);
  });

  test("有序列表保留原文起始编号", () => {
    const { container } = render(<Markdown text={"5. 第五步\n6. 第六步"} />);
    expect(container.querySelector("ol")!.getAttribute("start")).toBe("5");
  });
});

describe("行内代码智能断行", () => {
  test("短代码不插断点", () => {
    expect(softBreaks("lark-cli", "k")).toEqual(["lark-cli"]);
  });

  test("长代码只在分隔符后插 <wbr>，文字完整不丢", () => {
    const src = "{{REPORTS_DIR}}/pr/<platform>-<owner>-<repo>-<number>.review.json";
    const { container } = render(<Markdown text={`保存到 \`${src}\` 即可`} />);
    const code = container.querySelector("code")!;
    expect(code.textContent).toBe(src);
    const wbrs = code.querySelectorAll("wbr");
    expect(wbrs.length).toBeGreaterThan(3);
    // 每个断点前一个字符都是分隔符：不会把 review.json 切成 review.jso / n
    for (const w of Array.from(wbrs)) {
      const prev = w.previousSibling?.textContent ?? "";
      expect(prev.slice(-1)).toMatch(/[/\\.,:;_\-=|&?{}[\]()]/);
    }
  });

  test("JSON 片段在逗号、冒号后断开，枚举值 high|medium|low 不从单词中间切", () => {
    const json = '{"severity":"high|medium|low","status":"new|carried|resolved"}';
    const parts = softBreaks(json, "j").filter((p) => typeof p === "string") as string[];
    expect(parts.join("")).toBe(json);
    expect(parts).toContain('"high');
    expect(parts).toContain('medium');
  });
});
