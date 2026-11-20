// 新增机器人向导 4 步：① 飞书应用（扫码创建 / 手填）+ 名称 + 引擎 → ② 模板与技能 → ③ 绑定群 / 自动接入 / 谁能触发 → ④ 完成，可立即启动。
import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowLeft, ArrowRight, Bot as BotIcon, Check, CircleCheck, Copy, ExternalLink, Hash, Info, LoaderCircle, Play, Plus, Smartphone, Trash2 } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  CODE_REVIEW_SKILLS,
  CODE_REVIEW_TEMPLATE,
  DEFAULT_SKILLS,
  cancelCreateFeishuApp,
  createErrorText,
  createFeishuApp,
  errText,
  listProjects,
  listSkills,
  listTemplates,
  saveBot,
  saveGroup,
  startBot,
  type Bot,
  type Engine,
  type Group,
  type SkillInfo,
  type TemplateInfo,
  type Trigger,
  type TriggerMode,
} from "../../api";
import { Switch } from "../common";
import { ChipMultiSelect, Checkbox, EmptyState, HelpTip, Modal, QrCode, Segmented } from "../ui";
import { TRIGGER_MODES } from "../tabs/ConfigTab";
import { AUTO_ONBOARD_DESC, defaultSlug, OWNER_HINT, TRIGGER_LABEL } from "../tabs/GroupsTab";
import { templateName } from "../tabs/OverviewTab";
import Steps from "./Steps";

export function newBot(): Bot {
  return {
    id: "",
    name: "",
    app_id: "",
    engine: "claude",
    start_with_client: true,
    timeout_minutes: 30,
    concurrency: 2,
    template: "bug-hunt",
    skills: [...DEFAULT_SKILLS],
    extra_deny: [],
    auto_onboard: true,
    groups: [],
    trigger_mode: "mention",
  };
}

/** 选模板时连带改默认值：选「代码审查」→ 群里发 PR 链接触发、技能 PR 审查 + 代码同步；从代码审查换回别的模板 → 恢复默认。 */
export function withTemplate(bot: Bot, template: string): Bot {
  if (template === CODE_REVIEW_TEMPLATE) return { ...bot, template, trigger_mode: "pr_link", skills: [...CODE_REVIEW_SKILLS] };
  if (bot.template === CODE_REVIEW_TEMPLATE) return { ...bot, template, trigger_mode: "mention", skills: [...DEFAULT_SKILLS] };
  return { ...bot, template };
}

const triggerModeName = (m: TriggerMode | undefined) => TRIGGER_MODES.find(([k]) => k === (m ?? "mention"))?.[1] ?? "@ 机器人";

const STEP_NAMES = ["飞书应用", "模板与技能", "绑定群", "完成"];
const TRIGGER_OPTIONS: [Trigger, string][] = [
  ["owner", TRIGGER_LABEL.owner],
  ["all", TRIGGER_LABEL.all],
];
const ENGINES: [Engine, string][] = [
  ["claude", "Claude Code"],
  ["codex", "Codex"],
];

function Seg<T extends string>({ value, options, onChange, label }: { value: T; options: [T, string][]; onChange: (v: T) => void; label: string }) {
  return <Segmented ariaLabel={label} size="md" value={value} onChange={onChange} options={options.map(([k, t]) => ({ value: k, label: t }))} />;
}

export default function NewBotWizard({
  onClose,
  onCreated,
  onGoProjects,
}: {
  onClose: () => void;
  onCreated: (bot: Bot, started: boolean) => void;
  /** 没有项目时「去添加项目」：关闭向导并跳到项目页 */
  onGoProjects?: () => void;
}) {
  const [step, setStep] = useState(1);
  const [form, setForm] = useState<Bot>(newBot);
  const [mode, setMode] = useState<"qr" | "manual">("qr");
  const [secret, setSecret] = useState("");
  const [attempt, setAttempt] = useState<string | null>(null);
  const [scanning, setScanning] = useState<string | null>(null);
  const [scanUrl, setScanUrl] = useState<string | null>(null);
  const [scanned, setScanned] = useState(false);
  const [templates, setTemplates] = useState<TemplateInfo[]>([]);
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [projectNames, setProjectNames] = useState<string[]>([]);
  const [trigger, setTrigger] = useState<Trigger>("owner");
  const [groups, setGroups] = useState<Group[]>([]);
  const [draft, setDraft] = useState<Group>({ chat_id: "", name: "", slug: "", projects: [], doc_url: null, trigger: "owner" });
  const [saved, setSaved] = useState<Bot | null>(null);
  /** 已经在后端建好的机器人（群可能还没保存完） */
  const [created, setCreated] = useState<Bot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    listTemplates().then(setTemplates).catch(() => {});
    listSkills().then(setSkills).catch(() => {});
    listProjects()
      .then((v) => setProjectNames(Object.keys(v.projects)))
      .catch(() => {});
  }, []);

  // 关闭向导时取消还在等待的扫码创建（只在卸载时）
  const scanningRef = useRef<string | null>(null);
  scanningRef.current = scanning;
  useEffect(
    () => () => {
      const id = scanningRef.current;
      if (id) void cancelCreateFeishuApp(id).catch(() => {});
    },
    [],
  );

  const set = <K extends keyof Bot>(k: K, v: Bot[K]) => setForm((f) => ({ ...f, [k]: v }));

  const scan = async () => {
    setError(null);
    const id = crypto.randomUUID();
    setScanning(id);
    setScanUrl(null);
    setCopied(false);
    try {
      const app = await createFeishuApp(id, setScanUrl);
      setForm((f) => ({ ...f, app_id: app.appId, name: f.name || app.botName || "" }));
      setAttempt(id);
      setScanned(true);
    } catch (e) {
      setError(createErrorText(e));
    } finally {
      setScanning(null);
      setScanUrl(null);
    }
  };

  const step1Ok = !!form.name.trim() && (mode === "manual" ? !!form.app_id.trim() && !!secret.trim() : scanned);

  const addDraft = () => {
    if (!draft.chat_id.trim()) return;
    setGroups((gs) => [...gs.filter((g) => g.chat_id !== draft.chat_id), { ...draft, name: draft.name.trim() || draft.chat_id }]);
    setDraft({ chat_id: "", name: "", slug: "", projects: [], doc_url: null, trigger: "owner" });
  };

  const create = async () => {
    setError(null);
    setBusy(true);
    try {
      // 审查 B7：机器人只建一次。保存群失败后重点「完成创建」时沿用已建好的机器人 id（只更新它、补保存群），
      // 不会再建一个同 App ID 的机器人。扫码的密钥第一次保存时已进凭据库，重试不再传 attempt。
      const bot = await saveBot(
        { ...form, id: created?.id ?? form.id, groups: [] },
        mode === "manual" ? secret.trim() : null,
        mode === "qr" && !created ? attempt : null,
      );
      setCreated(bot);
      for (const g of groups) {
        await saveGroup(bot.id, { ...g, trigger, slug: g.slug || defaultSlug(g.chat_id) });
      }
      setSecret("");
      setSaved(bot);
      setStep(4);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const finish = async (start: boolean) => {
    if (!saved) return;
    if (start) {
      try {
        await startBot(saved.id);
      } catch (e) {
        setError(errText(e));
        return;
      }
    }
    onCreated(saved, start);
  };

  const toggleSkill = (name: string) => set("skills", form.skills.includes(name) ? form.skills.filter((s) => s !== name) : [...form.skills, name]);

  let body: ReactNode = null;
  if (step === 1)
    body = (
      <>
        <div style={{ marginBottom: 14 }}>
          <Seg
            label="创建方式"
            value={mode}
            options={[
              ["qr", "扫码创建飞书应用"],
              ["manual", "手动填写 App ID / Secret"],
            ]}
            onChange={setMode}
          />
        </div>
        {mode === "qr" ? (
          <div className="card" style={{ padding: 14 }}>
            <div style={{ fontWeight: 600 }}>用飞书 App 扫码 / 在浏览器授权</div>
            <ol style={{ color: "var(--muted)", fontSize: 12, lineHeight: 1.9, paddingLeft: 18, margin: "6px 0 12px", listStyle: "decimal" }}>
              <li>自动在你的企业下创建「企业自建应用」</li>
              <li>自动开通机器人能力、事件订阅（长连接）与群消息权限</li>
              <li>管理员审批通过后即可使用</li>
            </ol>
            {scanned ? (
              <div className="flex items-center gap-2" style={{ color: "var(--success-fg)" }} data-testid="scan-done">
                <CircleCheck style={{ width: 15, height: 15 }} />
                已创建 · App ID <span className="mono">{form.app_id}</span>
              </div>
            ) : scanning ? (
              <div className="flex items-start" style={{ gap: 16 }} data-testid="scan-pending">
                <div className="qr-box">
                  {scanUrl ? (
                    <QrCode text={scanUrl} size={168} label="飞书授权二维码" />
                  ) : (
                    <div className="qr-placeholder">
                      <LoaderCircle className="spin" style={{ width: 18, height: 18 }} />
                      正在生成二维码…
                    </div>
                  )}
                </div>
                <div className="grid min-w-0 flex-1" style={{ gap: 10, alignContent: "start" }}>
                  <div style={{ fontWeight: 600 }}>打开飞书 App，扫一扫左侧二维码</div>
                  <div style={{ color: "var(--muted)", fontSize: 12, lineHeight: 1.7 }}>
                    在手机上确认创建后，这里会自动继续。用不了手机时，也可以在电脑浏览器里打开授权页完成。
                  </div>
                  <div className="flex items-center gap-2" style={{ color: "var(--muted)", fontSize: 12 }}>
                    <LoaderCircle className="spin" style={{ width: 14, height: 14 }} />
                    等待确认…
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {scanUrl && (
                      <>
                        <button type="button" className="btn btn-sm tr" onClick={() => void openUrl(scanUrl)}>
                          <ExternalLink />
                          在浏览器打开
                        </button>
                        <button
                          type="button"
                          className="btn btn-sm tr"
                          onClick={() => {
                            void navigator.clipboard
                              ?.writeText(scanUrl)
                              .then(() => setCopied(true))
                              .catch(() => {});
                          }}
                        >
                          <Copy />
                          {copied ? "已复制" : "复制链接"}
                        </button>
                      </>
                    )}
                    <button type="button" className="btn btn-ghost btn-sm tr" onClick={() => void cancelCreateFeishuApp(scanning)}>
                      取消创建
                    </button>
                  </div>
                </div>
              </div>
            ) : (
              <button type="button" className="btn btn-pri tr" onClick={() => void scan()}>
                <Smartphone />
                开始扫码创建
              </button>
            )}
          </div>
        ) : (
          <div className="grid" style={{ gap: 12 }}>
            <div>
              <label className="lbl" htmlFor="wz-appid">
                App ID
              </label>
              <input id="wz-appid" className="inp mono" placeholder="cli_xxxxxxxxxxxxxxxx" value={form.app_id} onChange={(e) => set("app_id", e.target.value.trim())} />
            </div>
            <div>
              <label className="lbl" htmlFor="wz-secret">
                App Secret
              </label>
              <input id="wz-secret" className="inp mono" type="password" autoComplete="off" placeholder="32 位密钥" value={secret} onChange={(e) => setSecret(e.target.value)} />
            </div>
            <div className="flex items-center gap-1" style={{ color: "var(--muted)", fontSize: 12 }}>
              <Info style={{ width: 12, height: 12 }} /> 需在飞书开发者后台开启机器人能力，并把事件订阅方式设为「长连接」
            </div>
          </div>
        )}
        <div className="grid grid-cols-2" style={{ gap: 12, marginTop: 16 }}>
          <div>
            <label className="lbl" htmlFor="wz-name">
              机器人名称
            </label>
            <input id="wz-name" className="inp" placeholder="例如：线上问题助手" value={form.name} onChange={(e) => set("name", e.target.value)} />
          </div>
          <div>
            <span className="lbl">引擎</span>
            <Seg label="引擎" value={form.engine} options={ENGINES} onChange={(v) => set("engine", v)} />
          </div>
        </div>
        <div className="flex items-center gap-1.5" style={{ marginTop: 12, color: "var(--warning-fg)", fontSize: 12 }}>
          <Info style={{ width: 13, height: 13 }} />
          同一个飞书应用只能在一台电脑上连接，别和同事共用同一个 App ID
        </div>
      </>
    );
  if (step === 2)
    body = (
      <>
        <div style={{ fontWeight: 600, marginBottom: 8 }}>流程模板</div>
        <div className="grid grid-cols-3" style={{ gap: 8 }}>
          {templates.map((t) => (
            <button
              key={t.name}
              type="button"
              role="radio"
              aria-checked={form.template === t.name}
              className={`opt tr ${form.template === t.name ? "on" : ""}`}
              onClick={() => setForm((f) => withTemplate(f, t.name))}
            >
              <div style={{ fontWeight: 600 }}>{templateName(t.name)}</div>
              <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 4 }}>{t.builtin ? "内置" : "自建"}</div>
            </button>
          ))}
        </div>
        <div style={{ fontWeight: 600, margin: "16px 0 8px" }}>触发方式</div>
        <Seg label="触发方式" value={form.trigger_mode ?? "mention"} options={TRIGGER_MODES} onChange={(v) => set("trigger_mode", v)} />
        {(form.trigger_mode ?? "mention") !== "mention" && (
          <div className="flex items-center gap-1" style={{ color: "var(--muted)", fontSize: 12, marginTop: 6 }}>
            <Info style={{ width: 12, height: 12 }} /> 群里贴 Gitee / GitHub 的 PR 链接就自动审查，不用 @；需要飞书应用开通「获取群组中所有消息」权限
          </div>
        )}
        <div style={{ fontWeight: 600, margin: "16px 0 8px" }}>技能</div>
        <div className="grid grid-cols-2" style={{ gap: 8 }}>
          {skills.map((s) => (
            <Checkbox
              key={s.name}
              variant="card"
              checked={form.skills.includes(s.name)}
              ariaLabel={s.display_name}
              onChange={() => toggleSkill(s.name)}
              label={s.display_name}
              description={s.description}
            />
          ))}
        </div>
      </>
    );
  if (step === 3)
    body = (
      <>
        <div style={{ color: "var(--muted)", fontSize: 12, marginBottom: 12 }}>
          把机器人拉进飞书群后，打开「自动接入新群」，你在群里 @ 它就会自动登记；也可以在下面手动填 chat_id 先登记。
        </div>
        <div className="grid grid-cols-2" style={{ gap: 12 }}>
          <div className="card flex items-center" style={{ padding: "12px 14px", gap: 10 }}>
            <div className="flex-1">
              <div style={{ fontWeight: 600 }}>自动接入新群</div>
              <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>{AUTO_ONBOARD_DESC}</div>
            </div>
            <Switch on={form.auto_onboard} label="自动接入新群" onChange={(on) => set("auto_onboard", on)} />
          </div>
          <div className="card" style={{ padding: "12px 14px" }}>
            <div className="flex items-center gap-1.5" style={{ fontWeight: 600, marginBottom: 8 }}>
              谁能触发
              <HelpTip text={OWNER_HINT} label="仅机器人主人" />
            </div>
            <Seg label="谁能触发" value={trigger} options={TRIGGER_OPTIONS} onChange={setTrigger} />
          </div>
        </div>

        <div className="flex items-center justify-between" style={{ margin: "18px 0 8px" }}>
          <div style={{ fontWeight: 600 }}>
            手动登记的群 <span style={{ color: "var(--dim)", fontWeight: 400 }}>{groups.length}</span>
          </div>
          <span style={{ color: "var(--dim)", fontSize: 12 }}>开了自动接入可以跳过</span>
        </div>
        {groups.length === 0 ? (
          <div className="card" style={{ borderStyle: "dashed" }}>
            <EmptyState compact icon={Hash} title="还没有手动登记的群" desc="在下面填 chat_id 添加；也可以创建完成后在群里 @ 机器人自动登记" />
          </div>
        ) : (
          <div className="grid" style={{ gap: 8 }} data-testid="wizard-groups">
            {groups.map((g) => (
              <div key={g.chat_id} className="card flex items-center" style={{ gap: 12, padding: "10px 12px" }}>
                <div className="tile" style={{ width: 30, height: 30, borderRadius: 8 }}>
                  <Hash style={{ width: 15, height: 15, color: "var(--muted)" }} />
                </div>
                <div className="min-w-0 flex-1">
                  <div style={{ fontWeight: 600 }}>{g.name}</div>
                  <div className="mono" style={{ color: "var(--dim)", fontSize: 11 }}>
                    {g.chat_id}
                  </div>
                </div>
                <div className="flex flex-wrap justify-end gap-1">
                  {g.projects.length ? (
                    g.projects.map((p) => (
                      <span key={p} className="chip c-blue mono">
                        {p}
                      </span>
                    ))
                  ) : (
                    <span style={{ color: "var(--dim)", fontSize: 12 }}>用默认项目</span>
                  )}
                </div>
                <button type="button" className="btn btn-ghost btn-sm tr" aria-label={`移除 ${g.name}`} onClick={() => setGroups((gs) => gs.filter((x) => x.chat_id !== g.chat_id))}>
                  <Trash2 />
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="card" style={{ marginTop: 10, padding: "12px 14px" }} data-testid="wizard-add-group">
          <div style={{ fontWeight: 600, fontSize: 12, color: "var(--muted)", marginBottom: 10 }}>添加群</div>
          <div className="mini-form" style={{ gridTemplateColumns: "1.3fr 1fr auto" }}>
            <div>
              <label className="lbl" htmlFor="wz-chat-id">
                群 chat_id
              </label>
              <input
                id="wz-chat-id"
                className="inp mono"
                placeholder="oc_xxxxxxxxxxxxxxxx"
                value={draft.chat_id}
                aria-describedby="wz-chat-id-hint"
                onChange={(e) => setDraft({ ...draft, chat_id: e.target.value.trim() })}
                onKeyDown={(e) => e.key === "Enter" && addDraft()}
              />
            </div>
            <div>
              <label className="lbl" htmlFor="wz-group-name">
                群名称
              </label>
              <input
                id="wz-group-name"
                className="inp"
                placeholder="可选，留空用 chat_id"
                value={draft.name}
                onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                onKeyDown={(e) => e.key === "Enter" && addDraft()}
              />
            </div>
            <button type="button" className="btn btn-pri tr" disabled={!draft.chat_id} onClick={addDraft}>
              <Plus />
              添加群
            </button>
          </div>
          <div id="wz-chat-id-hint" className="field-hint">
            chat_id 以 oc_ 开头，可在飞书群设置里复制
          </div>
          <div style={{ marginTop: 12 }}>
            <div className="lbl flex items-center gap-1.5">
              负责项目（可多选）
              <HelpTip text="机器人在这个群里查问题时用哪些项目的代码和库；不选时用默认项目" label="负责项目" />
            </div>
            <ChipMultiSelect
              ariaLabel="负责项目"
              mono
              options={projectNames}
              value={draft.projects}
              onChange={(projects) => setDraft({ ...draft, projects })}
              empty={
                <span className="flex items-center gap-2" style={{ color: "var(--dim)", fontSize: 12 }}>
                  还没有项目
                  {onGoProjects && (
                    <button type="button" className="lnk" onClick={onGoProjects}>
                      去添加项目
                    </button>
                  )}
                </span>
              }
            />
          </div>
        </div>
      </>
    );
  if (step === 4 && saved)
    body = (
      <>
        <div style={{ textAlign: "center", padding: "14px 0 6px" }}>
          <div
            className="grid place-items-center"
            style={{ width: 60, height: 60, borderRadius: 16, margin: "0 auto", background: "var(--success-soft)", boxShadow: "0 0 30px color-mix(in srgb, var(--success) 20%, transparent)" }}
          >
            <CircleCheck style={{ width: 30, height: 30, color: "var(--success-fg)" }} />
          </div>
          <div style={{ fontSize: 17, fontWeight: 700, marginTop: 14 }}>「{saved.name}」已创建</div>
          <div style={{ color: "var(--muted)", marginTop: 4 }}>
            {saved.template === CODE_REVIEW_TEMPLATE ? "启动后在群里贴 PR 链接就能自动审查" : "启动后在群里 @ 它就能开始查 bug"}
          </div>
        </div>
        <div className="card" style={{ marginTop: 14 }} data-testid="wizard-summary">
          {(
            [
              ["飞书应用", saved.app_id],
              ["引擎", ENGINES.find(([k]) => k === saved.engine)?.[1] ?? saved.engine],
              ["流程模板", templateName(saved.template)],
              ["触发方式", triggerModeName(saved.trigger_mode)],
              ["技能", saved.skills.map((s) => skills.find((x) => x.name === s)?.display_name ?? s).join("、") || "无"],
              ["接入群", `${groups.length} 个${saved.auto_onboard ? " · 自动接入新群" : ""}`],
              ["谁能触发", TRIGGER_LABEL[trigger]],
            ] as [string, string][]
          ).map(([k, v]) => (
            <div key={k} className="flex" style={{ padding: "9px 14px", borderBottom: "1px solid var(--line)" }}>
              <span style={{ width: 90, color: "var(--muted)" }}>{k}</span>
              <span>{v}</span>
            </div>
          ))}
        </div>
      </>
    );

  // 机器人已经建好（哪怕群没保存完）就按「已创建」关闭，让列表刷新出来
  const close = () => {
    const done = saved ?? created;
    if (done) onCreated(done, false);
    else onClose();
  };

  return (
    <Modal
      title="新增机器人"
      icon={BotIcon}
      subtitle="接入一个飞书应用，选好模板与技能，再绑定要服务的群"
      width={720}
      onClose={close}
      closeDisabled={busy}
      dismissOnOverlay={step < 4 && !created}
      bodyStyle={{ padding: 0 }}
      footer={
        <div className="flex flex-1 justify-between">
          {step > 1 && step < 4 ? (
            <button type="button" className="btn tr" onClick={() => setStep(step - 1)}>
              <ArrowLeft />
              上一步
            </button>
          ) : (
            <span />
          )}
          {step < 3 ? (
            <button type="button" className="btn btn-pri tr" disabled={step === 1 && !step1Ok} onClick={() => setStep(step + 1)}>
              下一步 <ArrowRight />
            </button>
          ) : step === 3 ? (
            <button type="button" className="btn btn-pri tr" disabled={busy} onClick={() => void create()}>
              {busy ? <LoaderCircle className="spin" /> : <Check />}
              完成创建
            </button>
          ) : (
            <div className="flex gap-2">
              <button type="button" className="btn tr" onClick={() => void finish(false)}>
                稍后启动
              </button>
              <button type="button" className="btn btn-pri tr" onClick={() => void finish(true)}>
                <Play />
                立即启动
              </button>
            </div>
          )}
        </div>
      }
    >
      <div style={{ padding: "14px 20px", borderBottom: "1px solid var(--line)" }}>
        <Steps cur={step} names={STEP_NAMES} />
      </div>
      <div style={{ padding: "18px 20px" }}>
        {error && (
          <div className="alert-err" role="alert" style={{ marginBottom: 12 }}>
            {error}
          </div>
        )}
        {body}
      </div>
    </Modal>
  );
}
