// 数据源页：用户自己配置的库，给查 bug 的 Agent 用。两种查询方式：
// - 直连：客户端连库并强制只读（beetle --db-query），密码只存系统凭据库，Agent 看不到；
// - 自定义技能：交给本机已装的某个技能去查（如用户自己的查库技能），Beetle 只在运行时告诉 Agent 用哪个技能。
// 卡片网格 + 新建 / 编辑弹窗 + 删除确认。项目页「关联数据源」从这里多选。
import {
  Boxes,
  Check,
  Database,
  DatabaseZap,
  Leaf,
  Pencil,
  Plug,
  PlugZap,
  Plus,
  Puzzle,
  ShieldCheck,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  deleteDatasource,
  errText,
  listLocalSkills,
  listProjects,
  saveDatasource,
  testDatasource,
  type DataSource,
  type DbAccess,
  type DbKind,
  type LocalSkill,
  type ProjectsView,
} from "../../api";
import { PageHead, useToast } from "../common";
import { Checkbox, EmptyState, LoadingState, Modal, NumberInput, Segmented, Select, TextInput, Truncate, type SelectOption } from "../ui";

export const KIND_META: Record<DbKind, { label: string; Icon: LucideIcon; port: number | null; desc: string }> = {
  mysql: { label: "MySQL", Icon: Database, port: 3306, desc: "直连或技能查询" },
  postgres: { label: "PostgreSQL", Icon: DatabaseZap, port: 5432, desc: "直连或技能查询" },
  mongodb: { label: "MongoDB", Icon: Leaf, port: 27017, desc: "直连只支持 find" },
  other: { label: "其他", Icon: Boxes, port: null, desc: "只能用自定义技能查询" },
};

const KIND_OPTIONS: SelectOption<DbKind>[] = (Object.keys(KIND_META) as DbKind[]).map((k) => ({
  value: k,
  label: KIND_META[k].label,
  description: KIND_META[k].desc,
  icon: KIND_META[k].Icon,
}));

export const ALIAS_RE = /^[A-Za-z0-9_-]{1,64}$/;
const CUSTOM_SKILL = "__custom__";

/** 卡片 / 项目页悬停提示里的查询方式摘要。 */
export function accessSummary(ds: DataSource): string {
  if (ds.access === "direct") return `直连 ${ds.host}:${ds.port}/${ds.database}${ds.ssh ? " · 经 SSH 跳板" : ""}`;
  return `技能 ${ds.skill || "（未填）"}${ds.database ? ` · ${ds.database}` : ""}`;
}

// ───────────── 表单 ─────────────

interface Form {
  alias: string;
  kind: DbKind;
  description: string;
  access: DbAccess;
  host: string;
  port: string;
  user: string;
  password: string;
  database: string;
  timeout: string;
  maxRows: string;
  sshOn: boolean;
  sshHost: string;
  sshPort: string;
  sshUser: string;
  sshKey: string;
  skill: string;
  note: string;
}

const blankForm = (): Form => ({
  alias: "",
  kind: "mysql",
  description: "",
  access: "direct",
  host: "127.0.0.1",
  port: "3306",
  user: "",
  password: "",
  database: "",
  timeout: "5",
  maxRows: "200",
  sshOn: false,
  sshHost: "",
  sshPort: "22",
  sshUser: "root",
  sshKey: "",
  skill: "",
  note: "",
});

function formOf(alias: string, ds: DataSource): Form {
  const f = { ...blankForm(), alias, kind: ds.kind, description: ds.description, access: ds.access, database: ds.database };
  if (ds.access === "direct") {
    return {
      ...f,
      host: ds.host,
      port: String(ds.port),
      user: ds.user,
      timeout: String(ds.timeout_secs),
      maxRows: String(ds.max_rows),
      sshOn: !!ds.ssh,
      sshHost: ds.ssh?.host ?? "",
      sshPort: String(ds.ssh?.port ?? 22),
      sshUser: ds.ssh?.user ?? "root",
      sshKey: ds.ssh?.key_path ?? "",
    };
  }
  return { ...f, port: String(KIND_META[ds.kind].port ?? ""), skill: ds.skill, note: ds.note };
}

const intIn = (raw: string, min: number, max: number) => {
  const n = Number(raw);
  return raw.trim() !== "" && Number.isInteger(n) && n >= min && n <= max;
};

/** 第一个没填好的地方；null = 可以保存。 */
function problemOf(f: Form, isNew: boolean, aliases: string[]): string | null {
  if (!f.alias) return "填写别名";
  if (!ALIAS_RE.test(f.alias)) return "别名只能用字母、数字、下划线、短横线，最长 64 个字符";
  if (isNew && aliases.includes(f.alias)) return `别名 ${f.alias} 已存在`;
  if (f.access === "skill") return f.skill.trim() ? null : "选择或填写查询技能";
  if (!f.host.trim()) return "填写主机";
  if (!intIn(f.port, 1, 65535)) return "端口要在 1～65535 之间";
  if (!f.user.trim()) return "填写账号";
  if (!f.database.trim()) return "填写库名";
  if (!intIn(f.timeout, 1, 300)) return "超时要在 1～300 秒之间";
  if (!intIn(f.maxRows, 1, 10000)) return "最多行数要在 1～10000 之间";
  if (f.sshOn) {
    if (!f.sshHost.trim() || !f.sshUser.trim()) return "填写 SSH 跳板的主机和用户";
    if (!intIn(f.sshPort, 1, 65535)) return "SSH 端口要在 1～65535 之间";
    if (!f.sshKey.trim()) return "填写 SSH 私钥路径";
  }
  return null;
}

function toDataSource(f: Form): DataSource {
  const base = { kind: f.kind, description: f.description.trim() };
  if (f.access === "skill") return { ...base, access: "skill", skill: f.skill.trim(), database: f.database.trim(), note: f.note.trim() };
  return {
    ...base,
    access: "direct",
    host: f.host.trim(),
    port: Number(f.port),
    user: f.user.trim(),
    database: f.database.trim(),
    ssh: f.sshOn ? { host: f.sshHost.trim(), port: Number(f.sshPort), user: f.sshUser.trim(), key_path: f.sshKey.trim() } : null,
    timeout_secs: Number(f.timeout),
    max_rows: Number(f.maxRows),
  };
}

/** 同名技能（Claude Code、Codex 都装了）合成一项，说明里写装在哪。 */
function skillOptions(skills: LocalSkill[]): SelectOption[] {
  const by = new Map<string, { desc: string; where: string[] }>();
  for (const s of skills) {
    const cur = by.get(s.name) ?? { desc: s.description, where: [] };
    cur.where.push(s.location === "claude" ? "Claude Code" : "Codex");
    if (!cur.desc) cur.desc = s.description;
    by.set(s.name, cur);
  }
  const opts: SelectOption[] = [...by.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, v]) => ({ value: name, label: name, description: `${v.where.join("、")}${v.desc ? ` · ${v.desc}` : ""}`, icon: Puzzle }));
  opts.push({ value: CUSTOM_SKILL, label: "手动填写…", description: "技能不在列表里（例如装在别的位置）", icon: Pencil });
  return opts;
}

type TestState = { ok: boolean; text: string } | "testing" | null;

function TestResult({ state, testId }: { state: TestState; testId: string }) {
  if (!state) return null;
  if (state === "testing")
    return (
      <span className="chip c-mut" data-testid={testId}>
        <PlugZap />
        测试中…
      </span>
    );
  return (
    <span className={`chip ${state.ok ? "c-ok" : "c-err"}`} data-testid={testId} title={state.text} style={{ maxWidth: "100%", whiteSpace: "normal", height: "auto", minHeight: 22, padding: "2px 8px" }}>
      {state.ok ? <ShieldCheck /> : <Plug />}
      {state.text}
    </span>
  );
}

function DataSourceDialog({
  initialAlias,
  initial,
  aliases,
  localSkills,
  onClose,
  onSaved,
}: {
  /** null = 新建 */
  initialAlias: string | null;
  initial: DataSource | null;
  aliases: string[];
  /** null = 还在读取 / 读取失败（只能手填） */
  localSkills: LocalSkill[] | null;
  onClose: () => void;
  onSaved: (alias: string) => void;
}) {
  const isNew = initialAlias === null;
  const [f, setF] = useState<Form>(() => (initial && initialAlias ? formOf(initialAlias, initial) : blankForm()));
  const options = useMemo(() => skillOptions(localSkills ?? []), [localSkills]);
  // 用户点过「手动填写…」或选过列表项后以用户为准；否则看当前技能名在不在本机列表里（列表读完前不算手填）
  const [customPick, setCustomSkill] = useState<boolean | null>(null);
  const customSkill = customPick ?? (localSkills !== null && !!f.skill && !options.some((o) => o.value === f.skill));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<TestState>(null);
  /** 编辑一个原本就是直连的数据源：密码留空 = 不改 */
  const hasSavedPassword = !isNew && initial?.access === "direct";

  const set = (patch: Partial<Form>) => {
    setF((x) => ({ ...x, ...patch }));
    setTest(null);
  };
  const setKind = (kind: DbKind) => {
    const prevDefault = KIND_META[f.kind].port;
    const next: Partial<Form> = { kind };
    // 端口还是上一个类型的默认值（或空）时跟着换
    if (KIND_META[kind].port && (f.port === "" || Number(f.port) === prevDefault)) next.port = String(KIND_META[kind].port);
    if (kind === "other") next.access = "skill";
    set(next);
  };

  const problem = problemOf(f, isNew, aliases);
  const isDirect = f.access === "direct";

  const save = async () => {
    if (problem) return;
    setError(null);
    setBusy(true);
    try {
      await saveDatasource(f.alias, toDataSource(f), isDirect ? f.password : null);
      onSaved(f.alias);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const runTest = async () => {
    setTest("testing");
    try {
      setTest({ ok: true, text: await testDatasource(toDataSource(f), initialAlias, f.password) });
    } catch (e) {
      setTest({ ok: false, text: errText(e) });
    }
  };

  const accessOptions = [
    { value: "direct" as const, label: "直连", icon: Plug, title: "客户端连库并强制只读，Agent 看不到密码" },
    { value: "skill" as const, label: "自定义技能", icon: Puzzle, title: "交给本机已装的技能去查" },
  ].filter((o) => f.kind !== "other" || o.value === "skill");

  const skillValue = customSkill ? CUSTOM_SKILL : f.skill || null;

  return (
    <Modal
      title={isNew ? "新建数据源" : `编辑数据源 ${initialAlias}`}
      icon={Database}
      subtitle="Agent 查 bug 时只能查项目里关联的数据源，一律只读"
      width={640}
      onClose={onClose}
      closeDisabled={busy}
      testId="ds-dialog"
      footer={
        <>
          {isDirect && (
            <button type="button" className="btn tr" disabled={busy || !!problem || test === "testing"} onClick={() => void runTest()}>
              <PlugZap />
              测试连接
            </button>
          )}
          <span className="min-w-0 flex-1" style={{ color: "var(--dim)", fontSize: 12 }} data-testid="ds-problem">
            {problem ?? ""}
          </span>
          <button type="button" className="btn tr" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn btn-pri tr" disabled={busy || !!problem} onClick={() => void save()}>
            <Check />
            {busy ? "保存中…" : "保存"}
          </button>
        </>
      }
    >
      <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: 12 }}>
        <div>
          <label className="lbl" htmlFor="ds-alias">
            别名
          </label>
          <TextInput
            id="ds-alias"
            mono
            disabled={!isNew}
            placeholder="如 demo_main"
            title={isNew ? undefined : "别名不能修改；要改名请新建一个再删掉旧的"}
            value={f.alias}
            invalid={!!f.alias && !ALIAS_RE.test(f.alias)}
            onChange={(e) => set({ alias: e.target.value.trim() })}
          />
        </div>
        <div>
          <label className="lbl" htmlFor="ds-kind">
            类型
          </label>
          <Select id="ds-kind" ariaLabel="类型" value={f.kind} options={KIND_OPTIONS} onChange={setKind} />
        </div>
        <div style={{ gridColumn: "1 / -1" }}>
          <label className="lbl" htmlFor="ds-desc">
            用途说明（会写进运行时给 Agent 看）
          </label>
          <TextInput id="ds-desc" placeholder="如：订单、售后主库" value={f.description} onChange={(e) => set({ description: e.target.value })} />
        </div>
        <div style={{ gridColumn: "1 / -1" }}>
          <span className="lbl">查询方式</span>
          <Segmented ariaLabel="查询方式" size="md" value={f.access} options={accessOptions} onChange={(access) => set({ access })} testId="ds-access" />
          {f.kind === "other" && (
            <span style={{ color: "var(--dim)", fontSize: 12, marginLeft: 10 }}>「其他」类型客户端连不了，只能交给技能去查</span>
          )}
        </div>

        {isDirect ? (
          <>
            <div className="ds-note" style={{ gridColumn: "1 / -1" }}>
              <ShieldCheck />
              <span>
                客户端只放行单条 SELECT（Mongo 只允许 find），自动补 LIMIT、会话设为只读。建议用<b>只读账号</b>；密码只存在系统凭据库，配置文件和 Agent 都看不到。
              </span>
            </div>
            <div className="grid" style={{ gridColumn: "1 / -1", gridTemplateColumns: "2fr 1fr", gap: 12 }}>
              <div>
                <label className="lbl" htmlFor="ds-host">
                  主机
                </label>
                <TextInput id="ds-host" mono placeholder="127.0.0.1" value={f.host} onChange={(e) => set({ host: e.target.value.trim() })} />
              </div>
              <div>
                <label className="lbl" htmlFor="ds-port">
                  端口
                </label>
                <NumberInput id="ds-port" ariaLabel="端口" min={1} max={65535} value={f.port} invalid={!intIn(f.port, 1, 65535)} onChange={(port) => set({ port })} />
              </div>
            </div>
            <div>
              <label className="lbl" htmlFor="ds-user">
                账号
              </label>
              <TextInput id="ds-user" mono autoComplete="off" placeholder="readonly" value={f.user} onChange={(e) => set({ user: e.target.value.trim() })} />
            </div>
            <div>
              <label className="lbl" htmlFor="ds-password">
                密码
              </label>
              <TextInput
                id="ds-password"
                type="password"
                mono
                autoComplete="new-password"
                placeholder={hasSavedPassword ? "已保存，留空不修改" : "没有密码可留空"}
                value={f.password}
                onChange={(e) => set({ password: e.target.value })}
              />
            </div>
            <div>
              <label className="lbl" htmlFor="ds-database">
                库名
              </label>
              <TextInput id="ds-database" mono placeholder="demo" value={f.database} onChange={(e) => set({ database: e.target.value.trim() })} />
            </div>
            <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: 12 }}>
              <div>
                <label className="lbl" htmlFor="ds-timeout">
                  超时（秒）
                </label>
                <NumberInput id="ds-timeout" ariaLabel="超时" min={1} max={300} value={f.timeout} invalid={!intIn(f.timeout, 1, 300)} onChange={(timeout) => set({ timeout })} />
              </div>
              <div>
                <label className="lbl" htmlFor="ds-maxrows">
                  最多行数
                </label>
                <NumberInput
                  id="ds-maxrows"
                  ariaLabel="最多行数"
                  min={1}
                  max={10000}
                  step={50}
                  value={f.maxRows}
                  invalid={!intIn(f.maxRows, 1, 10000)}
                  onChange={(maxRows) => set({ maxRows })}
                />
              </div>
            </div>
            <div className="ds-ssh" style={{ gridColumn: "1 / -1" }} data-testid="ds-ssh">
              <Checkbox
                checked={f.sshOn}
                onChange={(sshOn) => set({ sshOn })}
                label="通过 SSH 跳板连接"
                description="库在内网时用系统 ssh 起隧道（ssh -N -L），查完即断开"
              />
              {f.sshOn && (
                <div className="grid" style={{ gridTemplateColumns: "2fr 1fr 1fr", gap: 12, marginTop: 12 }}>
                  <div>
                    <label className="lbl" htmlFor="ds-ssh-host">
                      SSH 主机
                    </label>
                    <TextInput id="ds-ssh-host" mono placeholder="跳板机地址" value={f.sshHost} onChange={(e) => set({ sshHost: e.target.value.trim() })} />
                  </div>
                  <div>
                    <label className="lbl" htmlFor="ds-ssh-port">
                      SSH 端口
                    </label>
                    <NumberInput id="ds-ssh-port" ariaLabel="SSH 端口" min={1} max={65535} value={f.sshPort} onChange={(sshPort) => set({ sshPort })} />
                  </div>
                  <div>
                    <label className="lbl" htmlFor="ds-ssh-user">
                      SSH 用户
                    </label>
                    <TextInput id="ds-ssh-user" mono value={f.sshUser} onChange={(e) => set({ sshUser: e.target.value.trim() })} />
                  </div>
                  <div style={{ gridColumn: "1 / -1" }}>
                    <label className="lbl" htmlFor="ds-ssh-key">
                      私钥路径
                    </label>
                    <TextInput id="ds-ssh-key" mono placeholder="C:/Users/me/.ssh/id_ed25519" value={f.sshKey} onChange={(e) => set({ sshKey: e.target.value.trim() })} />
                  </div>
                </div>
              )}
            </div>
          </>
        ) : (
          <>
            <div className="ds-note" style={{ gridColumn: "1 / -1" }}>
              <Puzzle />
              <span>Beetle 不调用这个技能，只在运行时告诉 Agent「这个库用哪个技能查」。技能要事先装在本机 Claude Code / Codex 里，查询仍须只读。</span>
            </div>
            <div>
              <label className="lbl" htmlFor="ds-skill">
                查询技能
              </label>
              <Select
                id="ds-skill"
                ariaLabel="查询技能"
                mono
                placeholder={localSkills === null ? "正在读取本机技能…" : "选择本机已装的技能"}
                value={skillValue}
                options={options}
                onChange={(v) => {
                  if (v === CUSTOM_SKILL) {
                    setCustomSkill(true);
                    set({ skill: "" });
                  } else {
                    setCustomSkill(false);
                    set({ skill: v });
                  }
                }}
              />
              {customSkill && (
                <TextInput
                  id="ds-skill-name"
                  aria-label="技能名"
                  mono
                  style={{ marginTop: 8 }}
                  placeholder="技能目录名，如 my-sql-skill"
                  value={f.skill}
                  onChange={(e) => set({ skill: e.target.value.trim() })}
                />
              )}
            </div>
            <div>
              <label className="lbl" htmlFor="ds-skill-db">
                库名（传给技能的名字）
              </label>
              <TextInput id="ds-skill-db" mono placeholder="可留空" value={f.database} onChange={(e) => set({ database: e.target.value.trim() })} />
            </div>
            <div style={{ gridColumn: "1 / -1" }}>
              <label className="lbl" htmlFor="ds-note">
                补充说明（给 Agent）
              </label>
              <textarea
                id="ds-note"
                className="inp"
                rows={3}
                placeholder="如：库名写 demo；这个技能需要先审批，结果在话题里"
                value={f.note}
                onChange={(e) => set({ note: e.target.value })}
              />
            </div>
          </>
        )}
      </div>
      {(test || error) && (
        <div className="grid" style={{ gap: 8, marginTop: 14 }}>
          <TestResult state={test} testId="ds-test-result" />
          {error && (
            <div className="alert-err" role="alert">
              {error}
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}

function DeleteDialog({ alias, usedBy, onClose, onDeleted }: { alias: string; usedBy: string[]; onClose: () => void; onDeleted: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(usedBy.length ? `数据源 ${alias} 被项目 ${usedBy.join("、")} 使用，先在项目里取消关联` : null);
  const remove = async () => {
    setError(null);
    setBusy(true);
    try {
      await deleteDatasource(alias);
      onDeleted();
    } catch (e) {
      setError(errText(e));
      setBusy(false);
    }
  };
  return (
    <Modal
      title={`删除数据源 ${alias}`}
      icon={Trash2}
      width={460}
      onClose={onClose}
      closeDisabled={busy}
      testId="ds-delete-dialog"
      footer={
        <>
          <button type="button" className="btn tr" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn btn-danger tr" disabled={busy || usedBy.length > 0} onClick={() => void remove()}>
            <Trash2 />
            {busy ? "删除中…" : "确认删除"}
          </button>
        </>
      }
    >
      <div style={{ fontSize: 13, lineHeight: 1.7 }}>
        删除后配置和系统凭据库里保存的密码会一起删掉，不能恢复。
      </div>
      {error && (
        <div className="alert-err" role="alert" style={{ marginTop: 12 }}>
          {error}
        </div>
      )}
    </Modal>
  );
}

function Field({ k, children }: { k: string; children: ReactNode }) {
  return (
    <>
      <span className="ds-k">{k}</span>
      <span className="ds-v mono">{children}</span>
    </>
  );
}

function DataSourceCard({
  alias,
  ds,
  usedBy,
  onEdit,
  onDelete,
}: {
  alias: string;
  ds: DataSource;
  usedBy: string[];
  onEdit: () => void;
  onDelete: () => void;
}) {
  const meta = KIND_META[ds.kind] ?? KIND_META.other;
  const [test, setTest] = useState<TestState>(null);
  const runTest = async () => {
    setTest("testing");
    try {
      setTest({ ok: true, text: await testDatasource(ds, alias, null) });
    } catch (e) {
      setTest({ ok: false, text: errText(e) });
    }
  };
  return (
    <div className="card ds-card tr" data-testid={`ds-${alias}`}>
      <div className="flex items-start" style={{ gap: 12, padding: "16px 16px 12px" }}>
        <div className="tile">
          <meta.Icon style={{ width: 18, height: 18, color: "var(--accent-text)" }} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="mono" style={{ fontWeight: 600, fontSize: 14 }}>
              {alias}
            </span>
            <span className="chip c-mut" style={{ height: 18, fontSize: 11 }}>
              {meta.label}
            </span>
            {ds.access === "direct" ? (
              <span className="chip c-blue" style={{ height: 18, fontSize: 11 }}>
                <Plug />
                直连 · 只读
              </span>
            ) : (
              <span className="chip c-warn" style={{ height: 18, fontSize: 11 }}>
                <Puzzle />
                技能查询
              </span>
            )}
          </div>
          <Truncate text={ds.description || "（没有说明）"} style={{ color: "var(--muted)", fontSize: 12, marginTop: 4 }} />
        </div>
        <button type="button" className="btn btn-sm tr" aria-label={`编辑 ${alias}`} onClick={onEdit}>
          <Pencil />
          编辑
        </button>
        <button type="button" className="btn btn-ghost btn-sm tr" aria-label={`删除 ${alias}`} onClick={onDelete}>
          <Trash2 />
        </button>
      </div>
      <div className="ds-kv">
        {ds.access === "direct" ? (
          <>
            <Field k="地址">
              {ds.host}:{ds.port}
            </Field>
            <Field k="库 / 账号">
              {ds.database} · {ds.user}
            </Field>
            {ds.ssh && (
              <Field k="SSH 跳板">
                {ds.ssh.user}@{ds.ssh.host}:{ds.ssh.port}
              </Field>
            )}
            <Field k="限制">
              {ds.timeout_secs} 秒超时 · 最多 {ds.max_rows} 行
            </Field>
          </>
        ) : (
          <>
            <Field k="查询技能">{ds.skill || "（未填）"}</Field>
            <Field k="库名">{ds.database || "—"}</Field>
            {ds.note && <Field k="补充说明">{ds.note}</Field>}
          </>
        )}
      </div>
      <div className="ds-foot">
        <span className="ds-k">项目</span>
        <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
          {usedBy.length ? (
            usedBy.map((p) => (
              <span key={p} className="chip c-mut mono" style={{ height: 18, fontSize: 11 }}>
                {p}
              </span>
            ))
          ) : (
            <span style={{ color: "var(--dim)" }}>没有项目关联</span>
          )}
        </span>
        {ds.access === "direct" ? (
          <button type="button" className="btn btn-sm tr" aria-label={`测试连接 ${alias}`} disabled={test === "testing"} onClick={() => void runTest()}>
            <PlugZap />
            测试连接
          </button>
        ) : (
          <span style={{ color: "var(--dim)" }} title="Beetle 不调用技能，由 Agent 运行时使用">
            由 Agent 调用技能
          </span>
        )}
      </div>
      {test && (
        <div style={{ padding: "0 16px 12px" }}>
          <TestResult state={test} testId={`ds-test-${alias}`} />
        </div>
      )}
    </div>
  );
}

export default function DataSourcesPage() {
  const toast = useToast();
  const [view, setView] = useState<ProjectsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [localSkills, setLocalSkills] = useState<LocalSkill[] | null>(null);
  /** undefined 不编辑；null 新建；字符串 = 编辑这个别名 */
  const [editing, setEditing] = useState<string | null | undefined>(undefined);
  const [deleting, setDeleting] = useState<string | null>(null);

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
  // 本机技能列表只给「查询技能」下拉用，读不到就只能手填，不报错
  useEffect(() => {
    if (editing === undefined || localSkills !== null) return;
    listLocalSkills()
      .then(setLocalSkills)
      .catch(() => setLocalSkills([]));
  }, [editing, localSkills]);

  const dbs = view?.dbs ?? {};
  const aliases = Object.keys(dbs).sort();
  const usedBy = (alias: string) => Object.keys(view?.projects ?? {}).filter((p) => view!.projects[p].dbs.includes(alias));

  return (
    <>
      <PageHead
        title="数据源"
        sub="Agent 查 bug 时能查的库：直连由客户端连库并强制只读，也可以交给你本机已装的技能去查"
        right={
          <button type="button" className="btn btn-pri tr" onClick={() => setEditing(null)}>
            <Plus />
            新建数据源
          </button>
        }
      />
      <div className="scroll flex-1" style={{ padding: "16px 24px" }}>
        {error && (
          <div className="alert-err" role="alert" style={{ marginBottom: 12 }}>
            {error}
          </div>
        )}
        {view === null && !error && <LoadingState text="正在读取数据源…" />}
        {view !== null && aliases.length === 0 && (
          <div className="card">
            <EmptyState
              icon={Database}
              title="还没有数据源"
              desc="添加后在项目里「关联数据源」，查 bug 时 Agent 就能只读查询这些库；不配也能用，Agent 只读代码"
              action={
                <button type="button" className="btn btn-pri btn-sm tr" onClick={() => setEditing(null)}>
                  <Plus />
                  新建数据源
                </button>
              }
            />
          </div>
        )}
        <div className="grid grid-cols-2" style={{ gap: 12 }}>
          {aliases.map((a) => (
            <DataSourceCard key={a} alias={a} ds={dbs[a]} usedBy={usedBy(a)} onEdit={() => setEditing(a)} onDelete={() => setDeleting(a)} />
          ))}
        </div>
      </div>
      {editing !== undefined && (
        <DataSourceDialog
          initialAlias={editing}
          initial={editing ? (dbs[editing] ?? null) : null}
          aliases={aliases}
          localSkills={localSkills}
          onClose={() => setEditing(undefined)}
          onSaved={(alias) => {
            toast(editing === null ? `已添加数据源 ${alias}` : `数据源 ${alias} 已保存`);
            setEditing(undefined);
            void reload();
          }}
        />
      )}
      {deleting && (
        <DeleteDialog
          alias={deleting}
          usedBy={usedBy(deleting)}
          onClose={() => setDeleting(null)}
          onDeleted={() => {
            toast(`已删除数据源 ${deleting}`);
            setDeleting(null);
            void reload();
          }}
        />
      )}
    </>
  );
}
