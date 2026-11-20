"""把一条 bug 分析追加到本群对应的飞书文档（本机 lark-cli 用户身份），并回读确认。

markdown 文件结构（第一行 "## " 开头，字段顺序固定）：

  ## <bug 标题>

  **复现步骤**

  1. ...

  **修复建议**

  1. ...（文件:行号）

  **提出人**：张三

  **提出时间**：2026-11-02 09:46

  python append_bug_doc.py reports/2026-11-02-xxx.md          # 文档地址取运行时配置里的 doc_url
  python append_bug_doc.py --doc <飞书文档链接> reports/xxx.md
"""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_shared"))
from bp_common import lark_json, load_runtime  # noqa: E402

REQUIRED = ["复现步骤", "修复建议", "提出人", "提出时间"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--doc", help="飞书文档链接，默认用运行时配置里本群的 doc_url")
    ap.add_argument("file")
    a = ap.parse_args()

    doc = a.doc or load_runtime().get("doc_url")
    if not doc:
        raise SystemExit("运行时配置里本群没有 doc_url，请让机器人主人在客户端里给本群配置记录文档")

    path = Path(a.file).resolve()
    text = path.read_text(encoding="utf-8").strip()
    first = text.splitlines()[0] if text else ""
    if not first.startswith("## "):
        raise SystemExit("第一行必须是 '## <bug 标题>'")
    missing = [k for k in REQUIRED if k not in text]
    if missing:
        raise SystemExit(f"缺少字段：{missing}")
    pos = [text.find(k) for k in REQUIRED]
    if pos != sorted(pos):
        raise SystemExit(f"字段顺序必须是 {REQUIRED}")
    title = first[3:].strip()

    # lark-cli 的 @file 只接受相对路径，所以在文件所在目录执行
    lark_json(["docs", "+update", "--as", "user", "--doc", doc, "--command", "append",
               "--doc-format", "markdown", "--content", f"@{path.name}"], cwd=str(path.parent))
    content = lark_json(["docs", "+fetch", "--as", "user", "--doc", doc])["data"]["document"]["content"]
    if title not in content:
        raise SystemExit(f"回读没找到标题「{title}」，请检查文档")
    print(f"已写入并回读确认：{title}")
    print(doc)


if __name__ == "__main__":
    main()
