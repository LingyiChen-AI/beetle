// 技能详情弹窗：头部是技能名、版本、说明、依赖、各位置的安装状态与安装 / 更新 / 卸载；
// 左边是技能目录的文件树（目录可折叠，按类型区分图标），右边查看文件内容（等宽、行号、简单着色；
// Markdown 默认渲染预览，可切到源码）。文件只读，后端限制在技能目录内、单文件 512KB。
import {
  Braces,
  ChevronRight,
  Download,
  Eye,
  File,
  FileCode,
  FileText,
  Folder,
  FolderOpen,
  Image,
  RefreshCw,
  Code,
  Trash2,
  X,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { catalogFiles, catalogReadFile, errText, SKILL_TARGETS, type CatalogEntry, type CatalogFile, type CatalogFileContent, type EnvItem } from "../../api";
import { EmptyState, LoadingState, Modal, Segmented, Tooltip } from "../ui";
import { CodeView, langOf } from "./highlight";
import Markdown from "./Markdown";
import { fmtSize, missing, REQUIRE_LABEL, skillIcon, useSkillActions } from "./shared";

const CODE_EXT = new Set(["py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "sh", "bash", "ps1", "rs", "bat", "cmd"]);
const DATA_EXT = new Set(["json", "toml", "yaml", "yml", "ini", "cfg"]);
const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "svg", "ico", "webp"]);

export function fileIcon(name: string): { Icon: LucideIcon; color: string } {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "md" || ext === "markdown") return { Icon: FileText, color: "var(--info-fg)" };
  if (CODE_EXT.has(ext)) return { Icon: FileCode, color: "var(--success-fg)" };
  if (DATA_EXT.has(ext)) return { Icon: Braces, color: "var(--warning-fg)" };
  if (IMAGE_EXT.has(ext)) return { Icon: Image, color: "var(--accent-text)" };
  return { Icon: File, color: "var(--muted)" };
}

/** 父目录路径们：a/b/c.py → ["a", "a/b"] */
const ancestors = (path: string) => {
  const parts = path.split("/");
  return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
};

function FileTree({
  files,
  selected,
  collapsed,
  onToggle,
  onSelect,
}: {
  files: CatalogFile[];
  selected: string | null;
  collapsed: Set<string>;
  onToggle: (dir: string) => void;
  onSelect: (path: string) => void;
}) {
  const visible = files.filter((f) => !ancestors(f.path).some((a) => collapsed.has(a)));
  return (
    <div role="tree" aria-label="技能文件" className="ftree">
      {visible.map((f) => {
        const pad = 8 + f.depth * 14;
        if (f.is_dir) {
          const open = !collapsed.has(f.path);
          const Icon = open ? FolderOpen : Folder;
          return (
            <button
              key={f.path}
              type="button"
              role="treeitem"
              aria-expanded={open}
              aria-level={f.depth + 1}
              className="ftree-row tr"
              style={{ paddingLeft: pad }}
              data-testid={`tree-${f.path}`}
              onClick={() => onToggle(f.path)}
            >
              <ChevronRight className="ftree-chev" style={{ transform: open ? "rotate(90deg)" : undefined }} />
              <Icon style={{ color: "var(--warning-fg)" }} />
              <span className="ftree-name">{f.name}</span>
            </button>
          );
        }
        const { Icon, color } = fileIcon(f.name);
        const on = f.path === selected;
        return (
          <button
            key={f.path}
            type="button"
            role="treeitem"
            aria-selected={on}
            aria-level={f.depth + 1}
            className={`ftree-row tr ${on ? "on" : ""}`}
            style={{ paddingLeft: pad + 16 }}
            data-testid={`tree-${f.path}`}
            onClick={() => onSelect(f.path)}
          >
            <Icon style={{ color }} />
            <span className="ftree-name">{f.name}</span>
            <span className="ftree-size mono">{fmtSize(f.size)}</span>
          </button>
        );
      })}
    </div>
  );
}

function Viewer({ name, path }: { name: string; path: string }) {
  const [file, setFile] = useState<CatalogFileContent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const lang = langOf(path);
  const [mode, setMode] = useState<"preview" | "source">(lang === "markdown" ? "preview" : "source");

  useEffect(() => {
    let alive = true;
    setFile(null);
    setError(null);
    catalogReadFile(name, path)
      .then((f) => alive && setFile(f))
      .catch((e) => alive && setError(errText(e)));
    return () => {
      alive = false;
    };
  }, [name, path]);

  const lines = file?.content ? file.content.split("\n").length : 0;

  // 切换文件或「预览 / 源码」时回到左上角：源码视图可能横向滚过，带到预览里会把内容截掉
  const viewRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = viewRef.current;
    if (el) {
      el.scrollLeft = 0;
      el.scrollTop = 0;
    }
  }, [path, mode]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="fview-h">
        {(() => {
          const { Icon, color } = fileIcon(path);
          return <Icon style={{ width: 15, height: 15, color, flex: "none" }} />;
        })()}
        <Tooltip content={path} onlyWhenTruncated className="mono min-w-0 flex-1 truncate" style={{ fontSize: 12 }}>
          {path}
        </Tooltip>
        {file && (
          <span className="mono" style={{ color: "var(--dim)", fontSize: 11, whiteSpace: "nowrap" }}>
            {fmtSize(file.size)}
            {file.content != null && ` · ${lines} 行`}
          </span>
        )}
        {lang === "markdown" && file?.content != null && (
          <Segmented
            ariaLabel="查看方式"
            value={mode}
            onChange={setMode}
            options={[
              { value: "preview", label: "预览", icon: Eye },
              { value: "source", label: "源码", icon: Code },
            ]}
          />
        )}
      </div>
      <div
        ref={viewRef}
        className="scroll min-h-0 flex-1"
        data-testid="file-view"
        style={lang === "markdown" && mode === "preview" ? { overflowX: "hidden" } : undefined}
      >
        {error && (
          <div className="alert-err" role="alert" style={{ margin: 14 }}>
            {error}
          </div>
        )}
        {!file && !error && <LoadingState />}
        {file?.binary && <EmptyState icon={File} title="二进制文件，不支持预览" desc={`${fmtSize(file.size)} · 可以到技能安装目录里用其它工具打开`} />}
        {file?.too_large && <EmptyState icon={FileText} title="文件太大，不在这里显示" desc={`${fmtSize(file.size)}，超过 512 KB 的查看上限`} />}
        {file?.content != null &&
          (lang === "markdown" && mode === "preview" ? (
            <Markdown text={file.content} />
          ) : (
            <CodeView text={file.content} lang={lang} testId="code-view" />
          ))}
      </div>
    </div>
  );
}

export default function SkillDetail({
  entry,
  env,
  onClose,
  onInstall,
  onChanged,
}: {
  entry: CatalogEntry;
  env: EnvItem[];
  onClose: () => void;
  /** 打开安装弹窗（选择安装位置） */
  onInstall: () => void;
  onChanged: () => Promise<void>;
}) {
  const { error, busy, update, uninstall } = useSkillActions(entry, onChanged);
  const [files, setFiles] = useState<CatalogFile[] | null>(null);
  const [filesError, setFilesError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const Icon = skillIcon(entry.name);

  useEffect(() => {
    let alive = true;
    catalogFiles(entry.name)
      .then((fs) => {
        if (!alive) return;
        setFiles(fs);
        const first = fs.find((f) => !f.is_dir && f.path.toLowerCase() === "skill.md") ?? fs.find((f) => !f.is_dir && f.depth === 0) ?? fs.find((f) => !f.is_dir);
        setSelected(first?.path ?? null);
      })
      .catch((e) => alive && setFilesError(errText(e)));
    return () => {
      alive = false;
    };
  }, [entry.name]);

  const fileCount = useMemo(() => files?.filter((f) => !f.is_dir).length ?? 0, [files]);
  const toggle = (dir: string) =>
    setCollapsed((s) => {
      const n = new Set(s);
      if (n.has(dir)) n.delete(dir);
      else n.add(dir);
      return n;
    });

  const header = (
    <div className="skd-h">
      <div className="flex items-start" style={{ gap: 12 }}>
        <div className="tile" style={{ width: 42, height: 42, borderRadius: 11, background: "var(--accent-soft)", boxShadow: "inset 0 0 0 1px var(--accent-ring)" }}>
          <Icon style={{ width: 20, height: 20, color: "var(--accent-text)" }} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span style={{ fontWeight: 700, fontSize: 16 }}>{entry.display_name}</span>
            <span className="chip c-mut mono" style={{ height: 20, fontSize: 11 }}>
              v{entry.version}
            </span>
            <span className="mono" style={{ color: "var(--dim)", fontSize: 12 }}>
              {entry.name}
            </span>
          </div>
          <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 4, lineHeight: 1.6 }}>{entry.description}</div>
        </div>
        <button type="button" className="btn btn-pri btn-sm tr" disabled={busy} onClick={onInstall}>
          <Download />
          安装
        </button>
        <button type="button" className="btn btn-ghost btn-sm tr" aria-label="关闭" onClick={onClose}>
          <X />
        </button>
      </div>
      <div className="flex flex-wrap items-center" style={{ gap: 8, marginTop: 12, fontSize: 12 }}>
        {entry.requires.length > 0 && (
          <span className="flex flex-wrap items-center gap-1" data-testid="detail-requires">
            <span style={{ color: "var(--dim)", marginRight: 2 }}>依赖</span>
            {entry.requires.map((r) => {
              const bad = missing(env, r);
              return (
                <span key={r} className={`chip ${bad ? "c-err" : "c-mut"}`} style={{ height: 20, fontSize: 11 }} title={bad ? "环境检查未通过，请到「环境」页处理" : undefined}>
                  {REQUIRE_LABEL[r] ?? r}
                  {bad ? "（缺少）" : ""}
                </span>
              );
            })}
          </span>
        )}
        <span className="flex-1" />
        {SKILL_TARGETS.map((t) => {
          const v = entry.installed[t.id];
          return (
            <span key={t.id} className="skd-target" data-testid={`detail-target-${t.id}`}>
              <span className={`dot ${v ? "st-running" : "st-stopped"}`} />
              <span>{t.label}</span>
              <span className="mono" style={{ color: v ? "var(--fg-soft)" : "var(--dim)" }}>
                {v ? `v${v}` : "未安装"}
              </span>
              {v && entry.update_available[t.id] && (
                <button type="button" className="btn btn-sm tr" style={{ height: 22 }} disabled={busy} aria-label={`更新 ${t.label}`} onClick={() => void update(t.id)}>
                  <RefreshCw />
                  更新到 v{entry.version}
                </button>
              )}
              {v && (
                <button type="button" className="btn btn-ghost btn-sm tr" style={{ height: 22 }} disabled={busy} aria-label={`从 ${t.label} 卸载`} onClick={() => uninstall(t.id)}>
                  <Trash2 />
                </button>
              )}
            </span>
          );
        })}
      </div>
      {error && (
        <div className="alert-err" role="alert" style={{ marginTop: 10 }}>
          {error}
        </div>
      )}
    </div>
  );

  return (
    <Modal ariaLabel={`${entry.display_name} 详情`} header={header} onClose={onClose} width={1080} height="calc(100vh - 80px)" bodyStyle={{ padding: 0, overflow: "hidden" }} bodyClassName="flex">
      <div className="skd-side">
        <div className="skd-side-h">
          文件
          <span className="mono" style={{ color: "var(--dim)" }}>
            {files ? fileCount : ""}
          </span>
        </div>
        <div className="scroll min-h-0 flex-1" style={{ padding: "4px 6px 10px" }}>
          {filesError && (
            <div className="alert-err" role="alert" style={{ margin: 8 }}>
              {filesError}
            </div>
          )}
          {!files && !filesError && <LoadingState />}
          {files && files.length === 0 && <EmptyState compact icon={Folder} title="技能目录是空的" />}
          {files && <FileTree files={files} selected={selected} collapsed={collapsed} onToggle={toggle} onSelect={setSelected} />}
        </div>
      </div>
      {selected ? (
        <Viewer key={selected} name={entry.name} path={selected} />
      ) : (
        <div className="grid flex-1 place-items-center">{files && <EmptyState icon={FileText} title="选择左侧的文件查看内容" />}</div>
      )}
    </Modal>
  );
}
