// 代码审查机器人的结果卡片：和后端 src-tauri/src/review_card.rs 的 render 同一套布局（标题、概要、结论、编号发现、底注、查看 PR）。
// 运行记录的 final_reply 对代码审查运行存的是解析后的 JSON，运行日志按卡片样式预览。

export type Severity = "high" | "medium" | "low";
export type FindingStatus = "new" | "carried" | "resolved";

export interface ReviewFinding {
  severity: Severity;
  status?: FindingStatus;
  text: string;
  /** 以下是 PR 评论里展开的详情（可缺省）；卡片只用 title 和 text */
  title?: string;
  locations?: string[];
  snippet?: string;
  details?: string[];
}

export interface ReviewResult {
  repo: string;
  pr: number;
  title?: string;
  conclusion?: string;
  findings: ReviewFinding[];
  additions?: number;
  deletions?: number;
  files?: number;
  author?: string;
  url?: string;
  head_sha?: string;
  coverage?: { total?: number; reviewed?: number; skipped?: { path: string; reason?: string }[] };
  /** 次要建议（PR 评论单列，卡片只提数量） */
  minor?: ReviewFinding[];
  checks?: { risks?: string[]; scope?: string[]; executed?: string[] };
}

const SEVERITY_LABEL: Record<Severity, string> = { high: "高", medium: "中", low: "低" };
const SEVERITY_ORDER: Record<Severity, number> = { high: 0, medium: 1, low: 2 };
const CONCLUSION_MAX = 400;
const FINDING_MAX = 300;

/** final_reply 是不是一份审查结果（JSON 对象，有 repo、pr、findings）；不是返回 null。 */
export function parseReviewReply(text: string | null | undefined): ReviewResult | null {
  const t = (text ?? "").trim();
  if (!t.startsWith("{")) return null;
  try {
    const v = JSON.parse(t) as Partial<ReviewResult>;
    if (typeof v !== "object" || v === null) return null;
    if (typeof v.repo !== "string" || !v.repo.trim() || typeof v.pr !== "number" || v.pr <= 0 || !Array.isArray(v.findings)) return null;
    const ok = v.findings.every((f) => f && typeof f.text === "string" && f.severity in SEVERITY_LABEL);
    return ok ? (v as ReviewResult) : null;
  } catch {
    return null;
  }
}

const statusOf = (f: ReviewFinding): FindingStatus => f.status ?? "new";

/** 卡片里一条发现的文字：有小标题时「标题：概括」，否则只有概括（和后端 finding_summary 一致）。 */
export function findingSummary(f: ReviewFinding): string {
  const title = (f.title ?? "").trim().replace(/[。：:]+$/, "");
  const text = f.text.trim();
  if (!title || text.startsWith(title)) return text;
  if (!text) return title;
  return `${title}：${text}`;
}

/** 还没处理的发现（新发现 + 上一轮遗留），按严重度从高到低，同级保持原顺序。 */
export function openFindings(r: ReviewResult): ReviewFinding[] {
  return r.findings
    .map((f, i) => ({ f, i }))
    .filter(({ f }) => statusOf(f) !== "resolved")
    .sort((a, b) => SEVERITY_ORDER[a.f.severity] - SEVERITY_ORDER[b.f.severity] || a.i - b.i)
    .map(({ f }) => f);
}

function truncate(s: string, max: number): string {
  const chars = Array.from(s.trim());
  return chars.length <= max ? chars.join("") : chars.slice(0, max).join("") + "…";
}

const oneLine = (s: string, max: number) => truncate(s.split(/\s+/).filter(Boolean).join(" "), max);

export function cardTitle(r: ReviewResult): string {
  const n = openFindings(r).length;
  return n === 0 ? `代码审查 ${r.repo} #${r.pr}，未发现问题` : `代码审查 ${r.repo} #${r.pr}，${n} 项发现`;
}

function statusNote(f: ReviewFinding, incremental: boolean): string {
  switch (statusOf(f)) {
    case "carried":
      return "（上一轮遗留，未处理）";
    case "resolved":
      return "（上一轮发现，已处理）";
    default:
      return incremental ? "（新发现）" : "";
  }
}

export interface CardLine {
  /** 行的种类：PR 标题 / 灰色概要 / 小标题 / 正文 / 一条发现 / 已处理的发现 / 分隔线 / 底注 / 链接 */
  kind: "title" | "meta" | "heading" | "text" | "finding" | "resolved" | "divider" | "note" | "link";
  text: string;
  /** kind = finding / resolved 时：严重度 */
  severity?: Severity;
  /** kind = finding 时：序号（从 1 开始） */
  index?: number;
}

/** 卡片正文逐块（和后端卡片 render 一一对应，只是没有 markdown 记号）。 */
export function cardLines(r: ReviewResult): CardLine[] {
  const lines: CardLine[] = [];
  if (r.title?.trim()) lines.push({ kind: "title", text: oneLine(r.title, 120) });
  const meta = [`${r.repo} #${r.pr}`];
  if (r.author?.trim()) meta.push(`作者 ${oneLine(r.author, 40)}`);
  meta.push(`+${r.additions ?? 0} / -${r.deletions ?? 0}`, `${r.files ?? 0} 个文件`);
  lines.push({ kind: "meta", text: meta.join(" · ") });
  if (r.conclusion?.trim()) {
    lines.push({ kind: "divider", text: "" }, { kind: "heading", text: "结论" }, { kind: "text", text: oneLine(r.conclusion, CONCLUSION_MAX) });
  }
  lines.push({ kind: "divider", text: "" });
  const open = openFindings(r);
  const incremental = r.findings.some((f) => statusOf(f) !== "new");
  if (open.length === 0) {
    lines.push({ kind: "heading", text: "未发现问题" });
  } else {
    lines.push({ kind: "heading", text: `发现 ${open.length} 项` });
    open.forEach((f, i) => lines.push({ kind: "finding", index: i + 1, severity: f.severity, text: statusNote(f, incremental) + oneLine(findingSummary(f), FINDING_MAX) }));
  }
  const resolved = r.findings.filter((f) => statusOf(f) === "resolved");
  if (resolved.length > 0) {
    lines.push({ kind: "heading", text: `上一轮已处理 ${resolved.length} 项` });
    for (const f of resolved) lines.push({ kind: "resolved", severity: f.severity, text: oneLine(f.text, FINDING_MAX) });
  }
  // 底注：次要建议数、审查覆盖（和后端卡片一样合成一行）
  const notes: string[] = [];
  const minor = Array.isArray(r.minor) ? r.minor.length : 0;
  if (minor > 0) notes.push(`另有 ${minor} 项次要建议，见 PR 评论`);
  const c = r.coverage;
  if (c && typeof c.total === "number") {
    let cov = `审查覆盖：${c.reviewed ?? 0}/${c.total} 个文件`;
    const skipped = c.skipped ?? [];
    if (skipped.length > 0) {
      const shown = skipped.slice(0, 3).map((f) => (f.reason?.trim() ? `${oneLine(f.path, 120)}:${oneLine(f.reason, 60)}` : oneLine(f.path, 120)));
      cov += `，跳过 ${skipped.length} 个（${shown.join("；")}${skipped.length > 3 ? " 等" : ""}）`;
    }
    notes.push(cov);
  }
  if (notes.length > 0) lines.push({ kind: "divider", text: "" }, { kind: "note", text: notes.join(" · ") });
  if (r.url?.trim()) lines.push({ kind: "link", text: r.url.trim() });
  return lines;
}

export const severityLabel = (s: Severity) => SEVERITY_LABEL[s];
