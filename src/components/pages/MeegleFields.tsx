// 项目表单里「建飞书项目工单」的配置：先确认飞书项目 CLI（meegle）已安装并登录，
// 再用 CLI 拉空间 → 工单类型 → 建单必填字段（下拉选项）→ 角色，选好写进 project.meegle；默认处理人按姓名 / 邮箱搜。
import { LogIn, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  errText,
  meegleForm,
  meegleSpaces,
  meegleStatus,
  meegleTypes,
  meegleUsers,
  type MeegleAiField,
  type MeegleForm,
  type MeegleSpace,
  type MeegleStatus,
  type MeegleTarget,
  type MeegleType,
  type MeegleUserHit,
} from "../../api";
import { useEnvFix } from "../../hooks/useEnvFix";
import { Select } from "../ui";
import { PersonPicker, type PersonHit } from "./PersonPicker";

export const emptyMeegle = (): MeegleTarget => ({
  project_key: "",
  simple_name: "",
  space_name: "",
  work_item_type: "issue",
  work_item_type_name: "缺陷",
  fields: [],
  ai_fields: [],
  assignee: null,
  role: null,
});

/** CLI 查询结果缓存：每次调用要一两秒，重开弹窗时直接用；点刷新清空。失败的不缓存。 */
const cache = new Map<string, Promise<unknown>>();
function cached<T>(key: string, fn: () => Promise<T>): Promise<T> {
  let hit = cache.get(key) as Promise<T> | undefined;
  if (!hit) {
    hit = fn();
    cache.set(key, hit);
    hit.catch(() => cache.delete(key));
  }
  return hit;
}
export const clearMeegleCache = () => cache.clear();

/** 默认用固定值的字段（工单模板这类和问题无关的）；其它下拉默认交给 AI 判断。 */
const FIXED_BY_DEFAULT = (key: string, ty: string) => key === "template" || ty.includes("template");

const aiOf = (f: MeegleForm["fields"][number]): MeegleAiField => ({
  field_key: f.field_key,
  field_name: f.field_name,
  field_type: f.field_type,
  options: f.options,
  fallback: f.default,
});

const isMulti = (ty: string) => ty.startsWith("multi") && ty !== "multi_text";

/** AI 判断的可选范围限定为 allowed（按当前选项刷新名称；全被删掉时恢复全部）；默认值不在范围内就不兜底。 */
export function narrowAi(f: MeegleForm["fields"][number], allowed: string[]): MeegleAiField {
  const kept = f.options.filter((o) => allowed.includes(o.id));
  const options = kept.length ? kept : f.options;
  return { ...aiOf(f), options, fallback: options.some((o) => o.id === f.default) ? f.default : null };
}

/** 表单加载后整理字段：已配置过的保持原样（固定值 / AI 判断）；新字段模板类用空间默认值固定，其它交给 AI 判断。
 *  AI 判断的字段顺便刷新可选项。只保留当前表单里需要选的字段。 */
export function syncFields(t: MeegleTarget, form: MeegleForm): Pick<MeegleTarget, "fields" | "ai_fields"> {
  const fields: MeegleTarget["fields"] = [];
  const ai_fields: MeegleAiField[] = [];
  const prevAi = t.ai_fields ?? [];
  for (const f of form.fields) {
    if (f.auto) continue;
    const old = t.fields.find((x) => x.field_key === f.field_key);
    if (old) {
      fields.push(old);
      continue;
    }
    const prev = prevAi.find((x) => x.field_key === f.field_key);
    if (prev || !FIXED_BY_DEFAULT(f.field_key, f.field_type)) {
      ai_fields.push(prev ? narrowAi(f, prev.options.map((o) => o.id)) : aiOf(f));
      continue;
    }
    const opt = f.options.find((o) => o.id === f.default) ?? f.options[0];
    if (opt) fields.push({ field_key: f.field_key, field_name: f.field_name, value: isMulti(f.field_type) ? JSON.stringify([opt.id]) : opt.id, label: opt.name });
  }
  return { fields, ai_fields };
}

/** 缺哪些必填字段（保存前提示）。 */
export function missingFields(t: MeegleTarget | null | undefined, form: MeegleForm | null): string[] {
  if (!t?.project_key) return ["空间"];
  if (!form) return [];
  const ai = t.ai_fields ?? [];
  return form.fields
    .filter((f) => !f.auto && !ai.some((x) => x.field_key === f.field_key) && !t.fields.some((x) => x.field_key === f.field_key && x.value))
    .map((f) => f.field_name);
}

export function MeegleFields({ value, k, onChange }: { value: MeegleTarget | null | undefined; k: string; onChange: (t: MeegleTarget) => void }) {
  const t = value ?? emptyMeegle();
  const [st, setSt] = useState<MeegleStatus | null>(null);
  const [spaces, setSpaces] = useState<MeegleSpace[] | null>(null);
  const [types, setTypes] = useState<MeegleType[] | null>(null);
  const [form, setForm] = useState<MeegleForm | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const { fix, running } = useEnvFix();

  /** 登录状态和空间列表并行拉；force = 点了刷新，先清缓存。 */
  const check = useCallback(async (force = false) => {
    if (force) {
      clearMeegleCache();
      setForm(null);
      setTypes(null);
    }
    setErr(null);
    const spacesP = cached("spaces", meegleSpaces).catch(() => null);
    try {
      const s = await cached("status", meegleStatus);
      setSt(s);
      if (s.logged_in) setSpaces(await spacesP);
    } catch (e) {
      setErr(errText(e));
    }
  }, []);
  useEffect(() => void check(), [check]);
  // 已知没登录才不拉；还在检测登录时就和检测并行拉，省掉一轮等待
  const may = st?.logged_in !== false;

  // 选了空间就拉工单类型；选了类型就拉必填字段与角色（都走缓存）
  useEffect(() => {
    if (!may || !t.project_key) return;
    let live = true;
    cached(`types:${t.project_key}`, () => meegleTypes(t.project_key))
      .then((v) => live && setTypes(v))
      .catch((e) => live && setErr(errText(e)));
    return () => {
      live = false;
    };
  }, [may, t.project_key]);
  useEffect(() => {
    if (!may || !t.project_key || !t.work_item_type) return;
    let live = true;
    setForm(null);
    cached(`form:${t.project_key}:${t.work_item_type}`, () => meegleForm(t.project_key, t.work_item_type))
      .then((f) => {
        if (!live) return;
        setForm(f);
        const role = t.role ?? (f.suggested_role ? (f.roles.find((r) => r.role_id === f.suggested_role) ?? null) : null);
        onChange({ ...t, ...syncFields(t, f), role });
      })
      .catch((e) => live && setErr(errText(e)));
    return () => {
      live = false;
    };
    // 只在空间 / 类型变化时重新拉；t 的其它字段变化不触发
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [may, t.project_key, t.work_item_type]);

  const ai = t.ai_fields ?? [];
  const setField = (key: string, name: string, value: string, label: string) =>
    onChange({
      ...t,
      ai_fields: ai.filter((x) => x.field_key !== key),
      fields: [...t.fields.filter((x) => x.field_key !== key), ...(value ? [{ field_key: key, field_name: name, value, label }] : [])],
    });
  /** AI 判断的可选范围里勾选 / 取消一个选项（至少留一个）。 */
  const toggleOpt = (f: MeegleForm["fields"][number], id: string) => {
    const cur = ai.find((x) => x.field_key === f.field_key)?.options.map((o) => o.id) ?? f.options.map((o) => o.id);
    const next = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id];
    if (next.length === 0) return;
    onChange({ ...t, ai_fields: ai.map((x) => (x.field_key === f.field_key ? narrowAi(f, next) : x)) });
  };
  /** 切换某字段由 AI 判断 / 固定值；切到固定值时先用空间默认值占上。 */
  const setAi = (f: MeegleForm["fields"][number], on: boolean) => {
    if (on) {
      onChange({ ...t, fields: t.fields.filter((x) => x.field_key !== f.field_key), ai_fields: [...ai.filter((x) => x.field_key !== f.field_key), aiOf(f)] });
      return;
    }
    const opt = f.options.find((o) => o.id === f.default) ?? f.options[0];
    onChange({
      ...t,
      ai_fields: ai.filter((x) => x.field_key !== f.field_key),
      fields: [
        ...t.fields.filter((x) => x.field_key !== f.field_key),
        ...(opt ? [{ field_key: f.field_key, field_name: f.field_name, value: isMulti(f.field_type) ? JSON.stringify([opt.id]) : opt.id, label: opt.name }] : []),
      ],
    });
  };

  const searchUsers = useCallback(
    async (q: string): Promise<PersonHit[]> => (await meegleUsers(q)).map((u: MeegleUserHit) => ({ id: u.user_key, name: u.name, sub: u.email })),
    [],
  );

  const busy = running.has("meegle_cli") || running.has("meegle_login");
  const missing = form ? missingFields(t, form) : [];
  const autos = form?.fields.filter((f) => f.auto) ?? [];
  const pickable = form?.fields.filter((f) => !f.auto) ?? [];

  const status = !st ? (
    <span className="chip c-mut">检测中…</span>
  ) : st.logged_in ? (
    <span className="chip c-ok" data-testid={`meegle-login-${k}`} title={st.host ?? undefined}>
      已登录{st.user ? ` · ${st.user}` : ""}
    </span>
  ) : (
    <span className="chip c-err" data-testid={`meegle-login-${k}`}>
      {st.installed ? "还没有登录" : "还没有安装"}
    </span>
  );

  return (
    <SubCard
      title="飞书项目工单"
      testId={`meegle-${k}`}
      extra={
        <>
          {status}
          <button type="button" className="btn btn-sm tr" aria-label="重新检查" title="重新检查登录状态，并重新读取空间和字段" onClick={() => void check(true)}>
            <RefreshCw />
          </button>
        </>
      }
    >
      {st && !st.logged_in && (
        <div className="flex flex-wrap items-center gap-2" style={{ fontSize: 12, color: "var(--muted)" }}>
          <button type="button" className="btn btn-pri btn-sm tr" disabled={busy} onClick={() => void fix(st.installed ? "meegle_login" : "meegle_cli").then(() => check(true))}>
            <LogIn />
            {busy ? "处理中…" : st.installed ? "去登录" : "安装飞书项目 CLI"}
          </button>
          {st.installed ? "会打开一个终端窗口，按提示在浏览器里授权，完成后点右上角刷新" : "安装完成后点「去登录」"}
        </div>
      )}
      {may && (
        <>
          <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: 12 }}>
            <div>
              <label className="lbl" htmlFor={`mg-space-${k}`}>
                空间
              </label>
              <Select
                id={`mg-space-${k}`}
                testId={`mg-space-${k}`}
                placeholder={spaces === null ? "加载中…" : spaces.length ? "选择空间" : "没有可用空间（先在飞书项目里访问一次）"}
                value={t.project_key || null}
                options={(spaces ?? []).map((s) => ({ value: s.project_key, label: s.name || s.project_key }))}
                onChange={(pk) => {
                  const s = spaces?.find((x) => x.project_key === pk);
                  setTypes(null);
                  onChange({ ...emptyMeegle(), project_key: pk, simple_name: s?.simple_name ?? "", space_name: s?.name ?? "", assignee: t.assignee });
                }}
              />
            </div>
            <div>
              <label className="lbl" htmlFor={`mg-type-${k}`}>
                工单类型
              </label>
              <Select
                id={`mg-type-${k}`}
                testId={`mg-type-${k}`}
                disabled={!t.project_key}
                placeholder={types === null ? "先选空间" : "选择类型"}
                value={t.work_item_type || null}
                options={(types ?? []).map((x) => ({ value: x.type_key, label: x.name }))}
                onChange={(tk) => onChange({ ...t, work_item_type: tk, work_item_type_name: types?.find((x) => x.type_key === tk)?.name ?? tk, fields: [], ai_fields: [], role: null })}
              />
            </div>
          </div>

          {!form && t.project_key && (
            <div aria-busy="true">
              <div className="lbl" style={{ marginBottom: 6 }}>
                必填字段 <span style={{ color: "var(--dim)", fontWeight: 400 }}>正在读取…</span>
              </div>
              <div style={{ border: "1px solid var(--line)", borderRadius: 8 }}>
                {Array.from({ length: Math.max(3, t.fields.length + ai.length) }, (_, i) => (
                  <div key={i} className="flex items-center" style={{ gap: 12, height: 48, padding: "0 12px", borderTop: i ? "1px solid var(--line)" : undefined }}>
                    <span className="skel" style={{ width: 72, height: 12 }} />
                    <span className="skel" style={{ width: 128, height: 24 }} />
                    <span className="skel flex-1" style={{ height: 12 }} />
                  </div>
                ))}
              </div>
            </div>
          )}
          {form && (
            <div>
              <div className="flex items-baseline" style={{ gap: 8, marginBottom: 6 }}>
                <span className="lbl" style={{ margin: 0 }}>
                  必填字段
                </span>
                {autos.length > 0 && (
                  <span style={{ fontSize: 12, color: "var(--dim)" }} data-testid={`mg-auto-${k}`}>
                    {autos.map((f) => f.field_name).join("、")} 由机器人自动填写
                  </span>
                )}
              </div>
              <div className="grid" style={{ border: "1px solid var(--line)", borderRadius: 8 }}>
                {pickable.map((f, i) => {
                  const cur = t.fields.find((x) => x.field_key === f.field_key);
                  const isAi = ai.some((x) => x.field_key === f.field_key);
                  const multi = isMulti(f.field_type);
                  const curId = cur ? (multi ? ((JSON.parse(cur.value) as string[])[0] ?? null) : cur.value) : null;
                  return (
                    <div
                      key={f.field_key}
                      className="grid items-center"
                      style={{ gridTemplateColumns: "96px 128px minmax(0, 1fr)", gap: 12, padding: "8px 12px", borderTop: i ? "1px solid var(--line)" : undefined, minHeight: 48 }}
                    >
                      <label htmlFor={`mg-f-${k}-${f.field_key}`} className="truncate" style={{ fontSize: 13 }} title={f.field_name}>
                        {f.field_name}
                      </label>
                      <div className="seg" role="group" aria-label={`${f.field_name}怎么填`}>
                        <button type="button" aria-pressed={isAi} data-testid={`mg-ai-${k}-${f.field_key}`} className={isAi ? "on" : ""} onClick={() => setAi(f, true)}>
                          AI 判断
                        </button>
                        <button type="button" aria-pressed={!isAi} className={!isAi ? "on" : ""} onClick={() => setAi(f, false)}>
                          固定值
                        </button>
                      </div>
                      {isAi ? (
                        f.options.length ? (
                          <div className="flex min-w-0 flex-wrap items-center" style={{ gap: 4, padding: "4px 0" }} data-testid={`mg-ai-note-${k}-${f.field_key}`}>
                            {f.options.map((o) => {
                              const aiF = ai.find((x) => x.field_key === f.field_key);
                              const on = !aiF || aiF.options.some((x) => x.id === o.id);
                              return (
                                <button
                                  key={o.id}
                                  type="button"
                                  aria-pressed={on}
                                  title={on ? "AI 可以选这个，点一下排除" : "已排除，点一下让 AI 可以选"}
                                  className={`chip tr ${on ? "c-blue" : "c-mut"}`}
                                  style={{ cursor: "pointer", height: 22, textDecoration: on ? undefined : "line-through", opacity: on ? 1 : 0.65 }}
                                  onClick={() => toggleOpt(f, o.id)}
                                >
                                  {o.name}
                                </button>
                              );
                            })}
                            {(() => {
                              const fb = ai.find((x) => x.field_key === f.field_key)?.fallback;
                              const name = f.options.find((o) => o.id === fb)?.name;
                              return name ? <span style={{ fontSize: 11, color: "var(--dim)", marginLeft: 4 }}>拿不准时用 {name}</span> : null;
                            })()}
                          </div>
                        ) : (
                          <div style={{ fontSize: 12, color: "var(--muted)" }} data-testid={`mg-ai-note-${k}-${f.field_key}`}>
                            按问题填写文字
                          </div>
                        )
                      ) : f.options.length === 0 ? (
                        <input id={`mg-f-${k}-${f.field_key}`} className="inp" value={cur?.value ?? ""} onChange={(e) => setField(f.field_key, f.field_name, e.target.value, e.target.value)} />
                      ) : (
                        <Select
                          id={`mg-f-${k}-${f.field_key}`}
                          testId={`mg-f-${k}-${f.field_key}`}
                          size="sm"
                          invalid={!cur}
                          value={curId}
                          options={f.options.map((o) => ({ value: o.id, label: o.name }))}
                          onChange={(id) => setField(f.field_key, f.field_name, multi ? JSON.stringify([id]) : id, f.options.find((o) => o.id === id)?.name ?? id)}
                        />
                      )}
                    </div>
                  );
                })}
                {pickable.length === 0 && <div style={{ padding: "10px 12px", fontSize: 12, color: "var(--muted)" }}>没有需要选的必填字段</div>}
              </div>
            </div>
          )}

          {t.project_key && (
            <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: 12 }}>
              <div>
                <label className="lbl" htmlFor={`mg-role-${k}`}>
                  处理人角色
                </label>
                <Select
                  id={`mg-role-${k}`}
                  testId={`mg-role-${k}`}
                  disabled={!form}
                  placeholder={form ? "不指定" : (t.role?.role_name ?? "加载中…")}
                  value={form ? (t.role?.role_id ?? null) : null}
                  options={(form?.roles ?? []).map((r) => ({ value: r.role_id, label: r.role_name }))}
                  onChange={(id) => onChange({ ...t, role: form?.roles.find((r) => r.role_id === id) ?? null })}
                />
              </div>
              <div>
                <label className="lbl" htmlFor={`mg-user-${k}`}>
                  默认处理人<span style={{ color: "var(--dim)", fontWeight: 400 }}>（可不填，交给人工分派）</span>
                </label>
                <PersonPicker
                  id={`mg-user-${k}`}
                  placeholder="输入姓名或邮箱搜索"
                  value={t.assignee ? { id: t.assignee.user_key, name: t.assignee.name } : null}
                  search={searchUsers}
                  onChange={(v) => onChange({ ...t, assignee: v ? { user_key: v.id, name: v.name } : null })}
                />
              </div>
            </div>
          )}

          {missing.length > 0 && (
            <div className="alert-err" style={{ fontSize: 12, padding: "6px 10px" }} data-testid={`meegle-missing-${k}`}>
              还没选：{missing.join("、")}（不选的话机器人建单会失败）
            </div>
          )}
        </>
      )}
      {err && (
        <div className="alert-err" style={{ fontSize: 12, padding: "6px 10px" }} data-testid={`meegle-error-${k}`}>
          {err}
        </div>
      )}
    </SubCard>
  );
}

/** 记录方式下的子配置卡片：标题行（标题 + 右侧状态 / 操作）+ 纵向排列的内容。 */
export function SubCard({ title, extra, testId, children }: { title: string; extra?: ReactNode; testId?: string; children: ReactNode }) {
  return (
    <section style={{ border: "1px solid var(--line)", borderRadius: 10, background: "var(--card)", marginTop: 12 }} data-testid={testId}>
      <header className="flex items-center" style={{ gap: 8, padding: "10px 14px", borderBottom: "1px solid var(--line)" }}>
        <span style={{ fontWeight: 600, fontSize: 13 }}>{title}</span>
        <span className="flex-1" />
        {extra}
      </header>
      <div className="grid" style={{ gap: 14, padding: 14 }}>
        {children}
      </div>
    </section>
  );
}
