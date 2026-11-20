// 轻量代码查看：按扩展名识别语言，逐行用正则切出注释 / 字符串 / 数字 / 关键字着色（不引入高亮库）。
// 只做单行级别的识别（跨行字符串、块注释不处理），对查看技能脚本足够。
import { Fragment, type CSSProperties, type ReactNode } from "react";

export type Lang = "markdown" | "python" | "js" | "json" | "shell" | "powershell" | "toml" | "yaml" | "rust" | "text";

const EXT: Record<string, Lang> = {
  md: "markdown",
  markdown: "markdown",
  py: "python",
  js: "js",
  mjs: "js",
  cjs: "js",
  ts: "js",
  tsx: "js",
  jsx: "js",
  json: "json",
  sh: "shell",
  bash: "shell",
  ps1: "powershell",
  toml: "toml",
  yaml: "yaml",
  yml: "yaml",
  rs: "rust",
};

export function langOf(path: string): Lang {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return EXT[ext] ?? "text";
}

const words = (s: string) => new Set(s.split(/\s+/).filter(Boolean));
const KEYWORDS: Partial<Record<Lang, Set<string>>> = {
  python: words(
    "def class return if elif else for while in not and or is import from as with try except finally raise pass break continue lambda yield global nonlocal assert del async await",
  ),
  js: words(
    "const let var function return if else for while do in of new class extends import from export default async await try catch finally throw switch case break continue typeof instanceof interface type enum implements this super",
  ),
  shell: words("if then else elif fi for while do done case esac in function return export local set echo exit"),
  powershell: words("function param if else elseif foreach for while return try catch finally throw switch begin process end"),
  rust: words("fn let mut pub use mod struct enum impl trait for in if else match return where as ref move async await loop while break continue crate self super"),
};
const LITERALS = words("true false null None True False undefined NaN");

/** 每种语言的行注释写法。 */
const COMMENT: Partial<Record<Lang, string>> = {
  python: "#.*$",
  shell: "#.*$",
  powershell: "#.*$",
  toml: "#.*$",
  yaml: "#.*$",
  js: "\\/\\/.*$",
  rust: "\\/\\/.*$",
};

const STRING = "\"(?:[^\"\\\\]|\\\\.)*\"?|'(?:[^'\\\\]|\\\\.)*'?|`(?:[^`\\\\]|\\\\.)*`?";
const NUMBER = "\\b\\d+(?:\\.\\d+)?\\b";
const WORD = "[A-Za-z_$][\\w$]*";

const cache = new Map<Lang, RegExp>();
function tokenRe(lang: Lang): RegExp {
  let re = cache.get(lang);
  if (!re) {
    const comment = COMMENT[lang];
    re = new RegExp(`${comment ? `(${comment})` : "(\\b\\B)"}|(${STRING})|(${NUMBER})|(${WORD})`, "g");
    cache.set(lang, re);
  }
  return re;
}

/** 一行代码切成带 className 的片段。 */
export function highlightLine(line: string, lang: Lang): ReactNode[] {
  if (lang === "text" || lang === "markdown" || !line) return [line];
  const re = tokenRe(lang);
  re.lastIndex = 0;
  const kws = KEYWORDS[lang];
  const out: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line))) {
    if (m[0] === "") {
      re.lastIndex++;
      continue;
    }
    const [text, comment, str, num, word] = m;
    let cls: string | null = null;
    if (comment) cls = "tk-c";
    else if (str) {
      // JSON / YAML 的键（后面跟冒号）和普通字符串分开着色
      cls = (lang === "json" || lang === "yaml") && /^\s*:/.test(line.slice(m.index + text.length)) ? "tk-key" : "tk-s";
    } else if (num) cls = "tk-n";
    else if (word) {
      if (LITERALS.has(word)) cls = "tk-l";
      else if (kws?.has(word)) cls = "tk-k";
      else if ((lang === "yaml" || lang === "toml") && /^\s*[:=]/.test(line.slice(m.index + text.length))) cls = "tk-key";
      else if (/^\s*\(/.test(line.slice(m.index + text.length))) cls = "tk-f";
    }
    if (!cls) continue;
    if (m.index > last) out.push(line.slice(last, m.index));
    out.push(
      <span key={m.index} className={cls}>
        {text}
      </span>,
    );
    last = m.index + text.length;
    // 注释吃掉整行剩余部分
    if (comment) break;
  }
  if (last < line.length) out.push(line.slice(last));
  return out;
}

/** 超过这么多行就不着色（只显示纯文本 + 行号），避免卡顿。 */
const HIGHLIGHT_MAX_LINES = 4000;

export function CodeView({ text, lang, testId }: { text: string; lang: Lang; testId?: string }) {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  const plain = lines.length > HIGHLIGHT_MAX_LINES;
  const width = String(lines.length).length;
  return (
    <div className="code-view mono" data-testid={testId} data-lang={lang} style={{ "--ln-w": `${width + 1}ch` } as CSSProperties}>
      {lines.map((l, i) => (
        <div key={i} className="cv-line">
          <span className="cv-ln" aria-hidden>
            {i + 1}
          </span>
          <span className="cv-code">{plain ? l : <Fragment>{highlightLine(l, lang)}</Fragment>}</span>
        </div>
      ))}
    </div>
  );
}
