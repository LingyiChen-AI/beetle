// 项目页：项目卡片（仓库、分支规则、规则文件、说明、关联数据源、记录方式）增删改；默认项目。
// 数据源本身在独立的「数据源」页管理，这里只多选关联。
import { Check, Database, FolderGit2, KeyRound, Pencil, Plus, Star, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  checkIssueToken,
  errText,
  listProjects,
  removeProject,
  saveProject,
  setDefaultProjects,
  type DataSource,
  type IssueTokenStatus,
  type Project,
  type ProjectsView,
  type RecordOpts,
} from "../../api";
import { DEFAULT_TOKEN_ENV, PLATFORM_LABEL, defaultRecord, issueTarget, parseRepo, platformOf } from "../../issueRepo";
import { PageHead, useToast } from "../common";
import { ChipMultiSelect, EmptyState, Modal, ToggleChip, Truncate, type ChipOption } from "../ui";
import { accessSummary } from "./DataSourcesPage";
import { MeegleFields, SubCard } from "./MeegleFields";
import { TaskFields } from "./TaskFields";

export const emptyProject = (): Project => ({
  repo: "",
  branch_glob: "release/*",
  rules_file: "AGENTS.md",
  description: "",
  dbs: [],
  record: { issue: false, feishu_doc: true, meegle: false, feishu_task: false },
  issue_repo: null,
  token_env: null,
  meegle: null,
  feishu_task: null,
  record_rule: null,
});

const recordOf = (p: Project): RecordOpts => {
  const r = p.record ?? defaultRecord(p.repo);
  return { ...r, meegle: r.meegle ?? false, feishu_task: r.feishu_task ?? false };
};

/** 提交前整理：记录方式补齐，issue 仓库 / token 变量名去空白，空的当没填。 */
function normalized(p: Project): Project {
  const blank = (v: string | null | undefined) => (v ?? "").trim() || null;
  return { ...p, record: recordOf(p), issue_repo: blank(p.issue_repo), token_env: blank(p.token_env), record_rule: blank(p.record_rule) };
}

/** 卡片上的记录方式摘要。 */
function recordSummary(p: Project): string[] {
  const r = recordOf(p);
  const out: string[] = [];
  if (r.issue) {
    const t = issueTarget(p);
    out.push(t ? `${PLATFORM_LABEL[t.platform]} issue · ${t.repo}` : "建 issue（仓库无法识别）");
  }
  if (r.meegle) out.push(p.meegle?.project_key ? `飞书项目 · ${p.meegle.space_name || p.meegle.project_key} · ${p.meegle.work_item_type_name || p.meegle.work_item_type}` : "飞书项目工单（未选空间）");
  if (r.feishu_task) out.push(p.feishu_task?.tasklist_name ? `飞书任务 · ${p.feishu_task.tasklist_name}` : "飞书任务");
  if (r.feishu_doc) out.push("飞书文档");
  if (p.record_rule?.trim() && out.length > 1) out.push("按记录规则挑选");
  return out;
}

function tokenText(st: IssueTokenStatus): string {
  if (!st.detected) return st.error ?? "未检测到";
  if (st.user) return `已检测到（账号 ${st.user}）`;
  return st.error ? `已检测到，但验证失败：${st.error}` : "已检测到";
}

const blankStatus = (error: string): IssueTokenStatus => ({
  platform: null,
  platform_label: null,
  repo: null,
  token_env: null,
  detected: false,
  user: null,
  error,
});

/** 表单里的「记录方式」：建 issue / 写飞书文档多选；建 issue 时显示识别出的平台与仓库、token 环境变量及检测结果。 */
function RecordFields({
  p,
  k,
  onChange,
  onToggle,
}: {
  p: Project;
  k: string;
  onChange: (next: Partial<Project>) => void;
  onToggle: () => void;
}) {
  const record = recordOf(p);
  const target = issueTarget(p);
  const base = parseRepo(p.repo);
  const basePlatform = platformOf(p.repo);
  const [tok, setTok] = useState<IssueTokenStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const check = async () => {
    setChecking(true);
    try {
      setTok(await checkIssueToken(normalized(p)));
    } catch (e) {
      setTok(blankStatus(`检测失败：${errText(e)}`));
    } finally {
      setChecking(false);
    }
  };
  const toggle = (key: keyof RecordOpts) => {
    onToggle();
    onChange({ record: { ...record, [key]: !record[key] } });
    setTok(null);
  };
  const picked = (["issue", "meegle", "feishu_task", "feishu_doc"] as const).filter((x) => record[x]).length;
  const chip = (key: keyof RecordOpts, label: string) => (
    <ToggleChip on={!!record[key]} onChange={() => toggle(key)}>
      {label}
    </ToggleChip>
  );
  return (
    <div style={{ gridColumn: "1 / -1" }} data-testid={`record-${k}`}>
      <span className="lbl">记录方式（可多选，找到问题后执行）</span>
      <div className="flex flex-wrap gap-1.5">
        {chip("issue", "建 issue")}
        {chip("meegle", "建飞书项目工单")}
        {chip("feishu_task", "建飞书任务")}
        {chip("feishu_doc", "写飞书文档")}
      </div>
      {picked > 1 && (
        <div style={{ marginTop: 10 }}>
          <label className="lbl" htmlFor={`pj-rule-${k}`}>
            记录规则（可选，用自然语言写什么情况用哪几种；不写就全部执行）
          </label>
          <textarea
            id={`pj-rule-${k}`}
            className="inp"
            rows={2}
            placeholder="例如：调研、需求类的建飞书任务；确认是 bug 的建飞书项目工单和 issue，再写飞书文档"
            value={p.record_rule ?? ""}
            onChange={(e) => onChange({ record_rule: e.target.value })}
          />
        </div>
      )}
      {record.issue && (
        <SubCard title={target ? `${PLATFORM_LABEL[target.platform]} issue` : "issue"} testId={`issue-${k}`}>
          <div className="grid" style={{ gridTemplateColumns: "1fr 1fr 1fr", gap: 12 }}>
          <div className="min-w-0">
            <span className="lbl">平台 · 仓库</span>
            <span className="mono break-all" style={{ fontSize: 12, color: target ? undefined : "var(--danger-fg)" }} data-testid={`issue-target-${k}`}>
              {target ? `${PLATFORM_LABEL[target.platform]} · ${target.repo}` : "仓库不是 gitee.com / github.com，请填 issue 仓库完整地址"}
            </span>
          </div>
          <div>
            <label className="lbl" htmlFor={`pj-issue-repo-${k}`}>
              issue 仓库（可改）
            </label>
            <input
              id={`pj-issue-repo-${k}`}
              className="inp mono"
              placeholder={base && basePlatform ? `${base.owner}/${base.repo}` : "https://gitee.com/owner/repo"}
              value={p.issue_repo ?? ""}
              onChange={(e) => {
                onChange({ issue_repo: e.target.value });
                setTok(null);
              }}
            />
          </div>
          <div>
            <label className="lbl" htmlFor={`pj-token-env-${k}`}>
              token 环境变量
            </label>
            <input
              id={`pj-token-env-${k}`}
              className="inp mono"
              placeholder={target ? DEFAULT_TOKEN_ENV[target.platform] : "GITEE_TOKEN"}
              value={p.token_env ?? ""}
              onChange={(e) => {
                onChange({ token_env: e.target.value.trim() });
                setTok(null);
              }}
            />
          </div>
          <div className="flex flex-wrap items-center gap-2" style={{ gridColumn: "1 / -1", fontSize: 12 }}>
            <button type="button" className="btn btn-sm tr" disabled={!target || checking} onClick={() => void check()}>
              <KeyRound />
              {checking ? "检测中…" : "检测 token"}
            </button>
            <span style={{ color: "var(--muted)" }}>
              读取环境变量 <span className="mono">{target?.tokenEnv ?? "—"}</span>（token 不保存到配置，也不会显示）
            </span>
            {tok && (
              <span data-testid={`token-status-${k}`} style={{ color: tok.detected && tok.user ? "var(--success-fg)" : "var(--danger-fg)" }}>
                {tokenText(tok)}
              </span>
            )}
          </div>
          </div>
        </SubCard>
      )}
      {record.meegle && <MeegleFields value={p.meegle} k={k} onChange={(meegle) => onChange({ meegle })} />}
      {record.feishu_task && <TaskFields value={p.feishu_task} k={k} onChange={(feishu_task) => onChange({ feishu_task })} />}
    </div>
  );
}

function ProjectForm({
  initialName,
  initial,
  dbs,
  onSave,
  onCancel,
  onGoDataSources,
}: {
  initialName: string | null;
  initial: Project;
  dbs: Record<string, DataSource>;
  onSave: (name: string, p: Project) => Promise<boolean>;
  onCancel: () => void;
  onGoDataSources: () => void;
}) {
  const [name, setName] = useState(initialName ?? "");
  const [p, setP] = useState<Project>(() => ({ ...initial, record: recordOf(initial) }));
  // 新项目：用户还没手动勾选记录方式前，「建 issue」跟着仓库地址自动勾（gitee / github 才勾）
  const [recordTouched, setRecordTouched] = useState(initialName !== null);
  const isNew = initialName === null;
  const set = <K extends keyof Project>(k: K, v: Project[K]) => setP((x) => ({ ...x, [k]: v }));
  const setRepo = (repo: string) =>
    setP((x) => ({ ...x, repo, record: recordTouched ? recordOf(x) : { ...recordOf(x), issue: platformOf(repo) !== null } }));
  const key = initialName ?? "new";
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    try {
      await onSave(name, normalized(p));
    } finally {
      setSaving(false);
    }
  };
  return (
    <Modal
      title={isNew ? "添加项目" : `编辑项目 ${initialName}`}
      icon={FolderGit2}
      subtitle="代码仓库、关联数据源和找到问题后的记录方式"
      width={780}
      height={Math.min(860, Math.round(window.innerHeight * 0.86))}
      onClose={onCancel}
      closeDisabled={saving}
      dismissOnOverlay={false}
      testId={`project-form-${key}`}
      footer={
        <>
          <button type="button" className="btn tr" disabled={saving} onClick={onCancel}>
            <X />
            取消
          </button>
          <button type="button" className="btn btn-pri tr" disabled={saving || !name || !p.repo.trim()} onClick={() => void save()}>
            <Check />
            {saving ? "保存中…" : "保存项目"}
          </button>
        </>
      }
    >
      <div className="grid" style={{ gridTemplateColumns: "1fr 2fr", gap: 12 }}>
        <div>
          <label className="lbl" htmlFor={`pj-name-${key}`}>
            项目名
          </label>
          <input
            id={`pj-name-${key}`}
            className="inp mono"
            disabled={!isNew}
            placeholder="例如 demo-service"
            value={name}
            onChange={(e) => setName(e.target.value.trim())}
          />
        </div>
        <div>
          <label className="lbl" htmlFor={`pj-repo-${key}`}>
            仓库地址
          </label>
          <input id={`pj-repo-${key}`} className="inp mono" placeholder="gitee.com/xxx/yyy.git" value={p.repo} onChange={(e) => setRepo(e.target.value)} />
        </div>
        <div>
          <label className="lbl" htmlFor={`pj-branch-${key}`}>
            分支规则（取最新）
          </label>
          <input id={`pj-branch-${key}`} className="inp mono" value={p.branch_glob} onChange={(e) => set("branch_glob", e.target.value)} />
        </div>
        <div>
          <label className="lbl" htmlFor={`pj-rules-${key}`}>
            规则文件
          </label>
          <input id={`pj-rules-${key}`} className="inp mono" value={p.rules_file} onChange={(e) => set("rules_file", e.target.value)} />
        </div>
        <div style={{ gridColumn: "1 / -1" }}>
          <label className="lbl" htmlFor={`pj-desc-${key}`}>
            说明
          </label>
          <input id={`pj-desc-${key}`} className="inp" placeholder="一句话说明这个项目是什么" value={p.description} onChange={(e) => set("description", e.target.value)} />
        </div>
        <div style={{ gridColumn: "1 / -1" }}>
          <span className="lbl">关联数据源（可多选，查 bug 时 Agent 只能查这些库）</span>
          <ChipMultiSelect
            ariaLabel="关联数据源"
            mono
            options={dsOptions(dbs, p.dbs)}
            value={p.dbs}
            onChange={(next) => set("dbs", next)}
            empty={
              <span className="flex items-center gap-2" style={{ color: "var(--dim)", fontSize: 12 }}>
                还没有数据源
                <button type="button" className="lnk" onClick={onGoDataSources}>
                  去数据源页添加
                </button>
              </span>
            }
          />
        </div>
        <RecordFields p={p} k={key} onChange={(next) => setP((x) => ({ ...x, ...next }))} onToggle={() => setRecordTouched(true)} />
      </div>
    </Modal>
  );
}

/** 多选项：已配置的数据源（悬停看查询方式），加上项目里引用了但已不存在的别名（标「已删除」，可取消）。 */
function dsOptions(dbs: Record<string, DataSource>, picked: string[]): ChipOption<string>[] {
  const opts: ChipOption<string>[] = Object.keys(dbs).map((a) => ({ value: a, title: `${dbs[a].kind} · ${accessSummary(dbs[a])}` }));
  for (const a of picked) if (!dbs[a]) opts.push({ value: a, label: `${a}（已删除）`, title: "数据源已不存在，点击取消关联" });
  return opts;
}

/** 没传跳转回调时（单独渲染）退回改地址栏 hash。 */
const hashGoDataSources = () => {
  window.location.hash = "page=datasources";
};

export default function ProjectsPage({ onGoDataSources = hashGoDataSources }: { onGoDataSources?: () => void }) {
  const toast = useToast();
  const [view, setView] = useState<ProjectsView>({ projects: {}, dbs: {}, default_projects: [] });
  const [editing, setEditing] = useState<string | null | undefined>(undefined); // undefined 不编辑；null 新建
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setView(await listProjects());
    } catch (e) {
      setError(errText(e));
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  const run = async (fn: () => Promise<unknown>, ok?: string): Promise<boolean> => {
    setError(null);
    try {
      await fn();
      if (ok) toast(ok);
      await reload();
      return true;
    } catch (e) {
      setError(errText(e));
      return false;
    }
  };

  const names = Object.keys(view.projects);
  const toggleDefault = (n: string) => {
    const on = view.default_projects.includes(n);
    void run(() => setDefaultProjects(on ? view.default_projects.filter((x) => x !== n) : [...view.default_projects, n]), on ? `已取消默认项目 ${n}` : `已设为默认项目 ${n}`);
  };

  return (
    <>
      <PageHead
        title="项目"
        sub="代码仓库与关联的数据源；群里选择「负责的项目」时从这里挑"
        right={
          <button type="button" className="btn btn-pri tr" onClick={() => setEditing(null)}>
            <Plus />
            添加项目
          </button>
        }
      />
      <div className="scroll grid flex-1 content-start" style={{ padding: "16px 24px", gap: 12 }}>
        {error && (
          <div className="alert-err" role="alert">
            {error}
          </div>
        )}
        {editing !== undefined && (editing === null || view.projects[editing]) && (
          <ProjectForm
            key={editing ?? "__new__"}
            initialName={editing}
            initial={editing === null ? emptyProject() : view.projects[editing]}
            dbs={view.dbs}
            onGoDataSources={() => {
              setEditing(undefined);
              onGoDataSources();
            }}
            onCancel={() => setEditing(undefined)}
            onSave={async (n, p) => {
              const ok = await run(() => saveProject(n, p), editing === null ? `已添加项目 ${n}` : "项目已保存");
              if (ok) setEditing(undefined);
              return ok;
            }}
          />
        )}
        {names.length === 0 && (
          <div className="card">
            <EmptyState
              icon={FolderGit2}
              title="还没有项目"
              desc="添加代码仓库和关联的数据源后，群里就能选择「负责的项目」"
              action={
                <button type="button" className="btn btn-pri btn-sm tr" onClick={() => setEditing(null)}>
                  <Plus />
                  添加项目
                </button>
              }
            />
          </div>
        )}
        {names.map((n) => {
          const p = view.projects[n];
          const isDefault = view.default_projects.includes(n);
          const records = recordSummary(p);
          return (
            <div key={n} className="card" style={{ padding: 16 }} data-testid={`project-${n}`}>
              <div className="flex items-center" style={{ gap: 12 }}>
                <div className="tile">
                  <FolderGit2 style={{ width: 18, height: 18, color: "var(--accent-text)" }} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="mono" style={{ fontWeight: 600, fontSize: 14 }}>
                    {n}
                  </div>
                  <Truncate text={p.description || "（没有说明）"} style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }} />
                </div>
                <button
                  type="button"
                  className={`chip tr ${isDefault ? "c-ok" : "c-mut"}`}
                  aria-pressed={isDefault}
                  title="自动接入的新群会关联默认项目；私聊可查全部项目，默认项目优先"
                  onClick={() => toggleDefault(n)}
                >
                  <Star />
                  {isDefault ? "默认项目" : "设为默认"}
                </button>
                <button type="button" className="btn btn-sm tr" aria-label={`编辑 ${n}`} onClick={() => setEditing(n)}>
                  <Pencil />
                  编辑
                </button>
                <button
                  type="button"
                  className="btn btn-ghost btn-sm tr"
                  aria-label={`删除 ${n}`}
                  onClick={() => window.confirm(`确定删除项目「${n}」吗？引用它的群需要另选项目。`) && void run(() => removeProject(n), `已删除项目 ${n}`)}
                >
                  <Trash2 />
                </button>
              </div>
              <div className="grid" style={{ gridTemplateColumns: "2fr 1fr 1fr 1.4fr 1.6fr", gap: 16, marginTop: 14, paddingTop: 12, borderTop: "1px solid var(--line)" }}>
                <div className="min-w-0">
                  <span className="lbl">仓库地址</span>
                  <span className="mono break-all" style={{ fontSize: 12 }}>
                    {p.repo}
                  </span>
                </div>
                <div>
                  <span className="lbl">分支规则</span>
                  <span className="mono" style={{ fontSize: 12 }}>
                    {p.branch_glob} 取最新
                  </span>
                </div>
                <div>
                  <span className="lbl">规则文件</span>
                  <span className="mono" style={{ fontSize: 12 }}>
                    {p.rules_file}
                  </span>
                </div>
                <div>
                  <span className="lbl">关联数据源</span>
                  {p.dbs.length ? (
                    p.dbs.map((d) => (
                      <span
                        key={d}
                        className={`chip mono ${view.dbs[d] ? "c-blue" : "c-err"}`}
                        style={{ marginRight: 4, marginBottom: 4 }}
                        title={view.dbs[d] ? `${view.dbs[d].kind} · ${accessSummary(view.dbs[d])}` : "数据源已不存在"}
                      >
                        <Database />
                        {d}
                      </span>
                    ))
                  ) : (
                    <span style={{ color: "var(--dim)", fontSize: 12 }}>无</span>
                  )}
                </div>
                <div className="min-w-0" data-testid={`record-summary-${n}`}>
                  <span className="lbl">记录方式</span>
                  {records.length ? (
                    records.map((r) => (
                      <span key={r} className="chip c-blue" style={{ marginRight: 4 }}>
                        {r}
                      </span>
                    ))
                  ) : (
                    <span style={{ color: "var(--dim)", fontSize: 12 }}>不记录</span>
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </>
  );
}
