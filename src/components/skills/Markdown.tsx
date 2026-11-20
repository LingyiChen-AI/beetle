// 轻量 Markdown 预览（只生成 React 元素，不用 innerHTML）：开头的 YAML front matter、标题、段落、
// 有序 / 无序列表、引用、代码块（带语法着色）、表格、分割线，行内的 `代码`、**粗体**、*斜体*、[链接](url)。
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ReactNode } from "react";
import { CodeView, langOf } from "./highlight";

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\[[^\]]+\]\([^)\s]+\))|(\*[^*\s][^*]*\*)/g;

// 行内代码的「智能断行」：在分隔符后插入 <wbr>，浏览器优先在这些位置换行，保持路径、JSON 键值、
// 命令片段的完整；只有单段本身比整行还长时，才由 CSS 的 overflow-wrap 兜底从中间断。
const BREAK_AFTER = /([/\\.,:;_\-=|&?{}[\]()])/;

export function softBreaks(text: string, keyBase: string): ReactNode[] {
  if (text.length <= 24) return [text];
  const out: ReactNode[] = [];
  text.split(BREAK_AFTER).forEach((part, i) => {
    if (!part) return;
    out.push(part);
    if (BREAK_AFTER.test(part)) out.push(<wbr key={`${keyBase}-w${i}`} />);
  });
  return out;
}

export function renderInline(text: string, keyBase = ""): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  // 每次调用用独立的正则对象：粗体内部会递归调用本函数，共享全局正则会把外层的 lastIndex 重置，导致死循环
  const re = new RegExp(INLINE.source, "g");
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const k = `${keyBase}-${m.index}`;
    const [all, code, bold, link, em] = m;
    if (code) out.push(<code key={k}>{softBreaks(code.slice(1, -1), k)}</code>);
    else if (bold) out.push(<strong key={k}>{renderInline(bold.slice(2, -2), k)}</strong>);
    else if (link) {
      const lm = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(link);
      const href = lm?.[2] ?? "";
      const external = /^https?:\/\//.test(href);
      out.push(
        external ? (
          <button key={k} type="button" className="lnk" onClick={() => void openUrl(href)}>
            {lm?.[1]}
          </button>
        ) : (
          <span key={k} className="md-ref" title={href}>
            {lm?.[1]}
          </span>
        ),
      );
    } else if (em) out.push(<em key={k}>{em.slice(1, -1)}</em>);
    else out.push(all);
    last = m.index + all.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** 拆出开头的 --- front matter（key: value 每行一项）。 */
export function splitFrontMatter(src: string): { meta: [string, string][]; body: string } {
  const text = src.replace(/\r\n?/g, "\n");
  const m = /^---\n([\s\S]*?)\n---\n?/.exec(text);
  if (!m) return { meta: [], body: text };
  const meta: [string, string][] = [];
  for (const line of m[1].split("\n")) {
    const kv = /^([\w-]+)\s*:\s*(.*)$/.exec(line);
    if (kv) meta.push([kv[1], kv[2].replace(/^["']|["']$/g, "")]);
    else if (meta.length && line.trim()) meta[meta.length - 1][1] += ` ${line.trim()}`;
  }
  return { meta, body: text.slice(m[0].length) };
}

const isTableSep = (l: string) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(l);
const cells = (l: string) =>
  l
    .trim()
    .replace(/^\||\|$/g, "")
    .split("|")
    .map((c) => c.trim());

const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;

type ListItem = { text: string; indent: number; marker: string };
type ListNode = { item: ListItem; children: ListItem[] };

/** 按缩进把列表项切成「本层项 + 各自的子项」：比第一项缩进更深的行都归到上一个本层项下面。 */
function groupLevel(items: ListItem[]): ListNode[] {
  const base = items[0]?.indent ?? 0;
  const nodes: ListNode[] = [];
  for (const it of items) {
    if (it.indent > base && nodes.length) nodes[nodes.length - 1].children.push(it);
    else nodes.push({ item: it, children: [] });
  }
  return nodes;
}

function checkbox(text: string) {
  return text.replace(/^\[( |x)\]\s+/i, (_, x: string) => (x.trim() ? "☑ " : "☐ "));
}

/** 嵌套列表：本层用第一项的符号决定有序 / 无序，有序列表从原文的第一个编号开始；子项递归成下一层列表。 */
function NestedList({ items, keyBase }: { items: ListItem[]; keyBase: string }) {
  const nodes = groupLevel(items);
  const first = nodes[0]?.item.marker ?? "-";
  const ordered = /\d/.test(first);
  const start = ordered ? parseInt(first, 10) : undefined;
  const List = ordered ? "ol" : "ul";
  return (
    <List start={start !== undefined && start !== 1 ? start : undefined}>
      {nodes.map((n, j) => (
        <li key={j}>
          {renderInline(checkbox(n.item.text), `${keyBase}-${j}`)}
          {n.children.length > 0 && <NestedList items={n.children} keyBase={`${keyBase}-${j}`} />}
        </li>
      ))}
    </List>
  );
}

export default function Markdown({ text }: { text: string }) {
  const { meta, body } = splitFrontMatter(text);
  const lines = body.split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let para: string[] = [];
  const flush = () => {
    if (para.length) {
      blocks.push(<p key={`p${i}-${blocks.length}`}>{renderInline(para.join(" "), `p${blocks.length}`)}</p>);
      para = [];
    }
  };

  while (i < lines.length) {
    const line = lines[i];
    const fence = /^\s*(```|~~~)\s*([\w+-]*)/.exec(line);
    if (fence) {
      flush();
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) code.push(lines[i++]);
      i++;
      const lang = langOf(`x.${fence[2] || "txt"}`);
      blocks.push(
        <div key={`c${i}`} className="md-code">
          <CodeView text={code.join("\n")} lang={lang} />
        </div>,
      );
      continue;
    }
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      flush();
      const level = h[1].length;
      const content = renderInline(h[2].replace(/\s+#+\s*$/, ""), `h${i}`);
      const Tag = `h${level}` as "h1";
      blocks.push(<Tag key={`h${i}`}>{content}</Tag>);
      i++;
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      blocks.push(<hr key={`hr${i}`} />);
      i++;
      continue;
    }
    if (/^\s*>/.test(line)) {
      flush();
      const q: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) q.push(lines[i++].replace(/^\s*>\s?/, ""));
      blocks.push(<blockquote key={`q${i}`}>{renderInline(q.join(" "), `q${i}`)}</blockquote>);
      continue;
    }
    if (line.includes("|") && i + 1 < lines.length && isTableSep(lines[i + 1])) {
      flush();
      const head = cells(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(cells(lines[i++]));
      blocks.push(
        <div key={`t${i}`} className="md-table scroll">
          <table>
            <thead>
              <tr>
                {head.map((c, j) => (
                  <th key={j}>{renderInline(c, `th${i}-${j}`)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r, ri) => (
                <tr key={ri}>
                  {head.map((_, j) => (
                    <td key={j}>{renderInline(r[j] ?? "", `td${i}-${ri}-${j}`)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      );
      continue;
    }
    const li = LIST_ITEM.exec(line);
    if (li) {
      flush();
      const items: ListItem[] = [];
      while (i < lines.length) {
        const m = LIST_ITEM.exec(lines[i]);
        if (m) {
          items.push({ text: m[3], indent: m[1].replace(/\t/g, "    ").length, marker: m[2] });
          i++;
        } else if (lines[i].trim() && /^\s{2,}/.test(lines[i]) && items.length) {
          // 列表项的续行
          items[items.length - 1].text += ` ${lines[i].trim()}`;
          i++;
        } else if (!lines[i].trim() && i + 1 < lines.length && LIST_ITEM.exec(lines[i + 1])) {
          // 列表项之间的空行：后面还是列表项就不断开（否则编号会从 1 重新开始）
          i++;
        } else break;
      }
      blocks.push(<NestedList key={`l${i}`} items={items} keyBase={`l${i}`} />);
      continue;
    }
    if (!line.trim()) {
      flush();
      i++;
      continue;
    }
    para.push(line.trim());
    i++;
  }
  flush();

  return (
    <div className="md-view" data-testid="md-preview">
      {meta.length > 0 && (
        <div className="md-meta" data-testid="md-front-matter">
          {meta.map(([k, v]) => (
            <div key={k} className="md-meta-row">
              <span className="mono">{k}</span>
              <span>{v}</span>
            </div>
          ))}
        </div>
      )}
      {blocks}
    </div>
  );
}
