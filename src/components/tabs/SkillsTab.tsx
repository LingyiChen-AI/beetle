// 技能与模板：勾选技能；选择流程模板；模板编辑器（等宽 textarea，可用变量提示，保存 / 恢复内置，保存错误就近显示）。
import { Check, Plus, RotateCcw, Save, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  errText,
  getTemplate,
  listSkills,
  listTemplates,
  resetTemplate,
  saveBot,
  saveTemplate,
  type BotView,
  type SkillInfo,
  type TemplateInfo,
} from "../../api";
import { useToast } from "../common";
import { Checkbox, EmptyState } from "../ui";
import { templateName } from "./OverviewTab";

export const TEMPLATE_VARS = ["GROUP_NAME", "CHAT_ID", "PROJECTS_BLOCK", "DBS_LIST", "DOC_URL", "SKILLS_DIR", "PYTHON", "NODE"];

// 技能勾选先保持最简（只列技能名 + 勾选），等技能安装中心的新设计。
export default function SkillsTab({ view, onChanged }: { view: BotView; onChanged: () => Promise<void> | void }) {
  const { bot } = view;
  const toast = useToast();
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [templates, setTemplates] = useState<TemplateInfo[]>([]);
  const [editing, setEditing] = useState(bot.template);
  const [content, setContent] = useState("");
  const [loaded, setLoaded] = useState("");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  /** 刚新建、还没保存过的模板名：不去后端读内容。 */
  const fresh = useRef<string | null>(null);

  const reloadTemplates = useCallback(async () => {
    try {
      setTemplates(await listTemplates());
    } catch (e) {
      toast(errText(e), "err");
    }
  }, [toast]);

  useEffect(() => {
    listSkills().then(setSkills).catch((e) => toast(errText(e), "err"));
    void reloadTemplates();
  }, [reloadTemplates, toast]);

  useEffect(() => {
    let alive = true;
    setSaveError(null);
    if (fresh.current === editing) return;
    getTemplate(editing)
      .then((t) => {
        if (!alive) return;
        setContent(t);
        setLoaded(t);
      })
      .catch((e) => alive && setSaveError(errText(e)));
    return () => {
      alive = false;
    };
  }, [editing]);

  const dirty = content !== loaded;
  const info = templates.find((t) => t.name === editing);

  const switchTo = (name: string) => {
    if (name === editing) return;
    if (dirty && !window.confirm("模板有未保存的修改，确定放弃吗？")) return;
    setEditing(name);
  };

  // 审查 S6：保存是整份机器人配置，连点两下时第二次会拿着刷新前的旧快照把第一次的勾选冲掉。
  // 保存到刷新完成之前禁用勾选 / 改模板，保证每次都基于最新的 view.bot。
  const [busy, setBusy] = useState(false);
  const saveSerial = async (next: typeof bot, ok?: string) => {
    if (busy) return;
    setBusy(true);
    try {
      await saveBot(next);
      await onChanged();
      if (ok) toast(ok);
    } catch (e) {
      toast(errText(e), "err");
    } finally {
      setBusy(false);
    }
  };

  const toggleSkill = (name: string) => {
    const on = bot.skills.includes(name);
    return saveSerial({ ...bot, skills: on ? bot.skills.filter((s) => s !== name) : [...bot.skills, name] });
  };

  const useTemplate = () => saveSerial({ ...bot, template: editing }, `本机器人改用模板「${templateName(editing)}」，下次运行生效`);

  const save = async () => {
    setSaveError(null);
    try {
      await saveTemplate(editing, content);
      if (fresh.current === editing) fresh.current = null;
      setLoaded(content);
      await reloadTemplates();
      toast("模板已保存，下次运行生效");
    } catch (e) {
      setSaveError(errText(e));
    }
  };

  const reset = async () => {
    if (!window.confirm(`确定把「${templateName(editing)}」恢复成内置版本吗？你的修改会被删除。`)) return;
    setSaveError(null);
    try {
      await resetTemplate(editing);
      const t = await getTemplate(editing);
      setContent(t);
      setLoaded(t);
      await reloadTemplates();
      toast("已恢复内置模板");
    } catch (e) {
      setSaveError(errText(e));
    }
  };

  const create = () => {
    const name = newName.trim();
    if (!name) return;
    setCreating(false);
    setNewName("");
    setTemplates((ts) => (ts.some((t) => t.name === name) ? ts : [...ts, { name, builtin: false, modified: false }]));
    fresh.current = name;
    setEditing(name);
    // 新模板从当前内容起步，保存后才落盘
    setLoaded("");
  };

  return (
    <div className="grid min-h-0 flex-1" style={{ gridTemplateColumns: "clamp(260px, 36%, 360px) minmax(0, 1fr)" }}>
      <div className="scroll" style={{ borderRight: "1px solid var(--line)", padding: 16 }}>
        <div className="flex items-center justify-between" style={{ marginBottom: 10 }}>
          <div style={{ fontWeight: 600 }}>
            技能{" "}
            <span style={{ color: "var(--dim)", fontWeight: 400 }}>
              已启用 {bot.skills.filter((s) => skills.some((x) => x.name === s)).length}/{skills.length}
            </span>
          </div>
        </div>
        {skills.length === 0 && <EmptyState compact title="没有可用的技能" desc="技能随客户端打包，读取失败时重启客户端再试" />}
        <div className="grid" style={{ gap: 6 }}>
          {skills.map((s) => (
            <Checkbox
              key={s.name}
              variant="card"
              checked={bot.skills.includes(s.name)}
              ariaLabel={s.display_name}
              disabled={busy}
              onChange={() => void toggleSkill(s.name)}
              label={s.display_name}
              description={s.description || undefined}
            />
          ))}
        </div>
      </div>

      <div className="flex min-h-0 flex-col" style={{ padding: 16 }}>
        <div className="flex items-center" style={{ gap: 10, marginBottom: 10 }}>
          <div style={{ fontWeight: 600 }}>流程模板</div>
          <div className="seg" role="radiogroup" aria-label="流程模板">
            {templates.map((t) => (
              <button key={t.name} type="button" role="radio" aria-checked={editing === t.name} className={`tr ${editing === t.name ? "on" : ""}`} onClick={() => switchTo(t.name)}>
                {templateName(t.name)}
                {t.name === bot.template && <span style={{ color: "var(--accent-text)" }}>·使用中</span>}
              </button>
            ))}
            <button type="button" className="tr" onClick={() => setCreating(true)}>
              <Plus />
              新建
            </button>
          </div>
          {creating && (
            <span className="flex items-center gap-1.5">
              <input
                className="inp mono"
                style={{ height: 26, width: 150 }}
                aria-label="新模板名"
                placeholder="英文名，如 my-flow"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && create()}
              />
              <button type="button" className="btn btn-sm tr" aria-label="确认新建" onClick={create}>
                <Check />
              </button>
              <button type="button" className="btn btn-ghost btn-sm tr" aria-label="取消新建" onClick={() => setCreating(false)}>
                <X />
              </button>
            </span>
          )}
          <div className="flex-1" />
          <span style={{ color: "var(--muted)", fontSize: 12 }}>
            生成到每个群工作目录的 <span className="mono">CLAUDE.md</span>
          </span>
        </div>
        <div style={{ color: "var(--muted)", fontSize: 12, marginBottom: 8 }}>
          {info ? (info.builtin ? (info.modified ? "内置 · 已修改" : "内置") : "自建") : "新模板（未保存）"} · 可用变量{" "}
          <span className="mono" style={{ color: "var(--accent-text)" }}>
            {TEMPLATE_VARS.map((v) => `{{${v}}}`).join(" ")}
          </span>{" "}
          按群自动替换
        </div>
        <textarea
          className="code scroll flex-1"
          style={{ minHeight: 280 }}
          spellCheck={false}
          aria-label="模板内容"
          value={content}
          onChange={(e) => setContent(e.target.value)}
        />
        {saveError && (
          <div className="alert-err" role="alert" style={{ marginTop: 8, fontSize: 12 }}>
            {saveError}
          </div>
        )}
        <div className="flex items-center justify-end gap-2" style={{ marginTop: 10 }}>
          {dirty && <span style={{ color: "var(--warning-fg)", fontSize: 12, marginRight: "auto" }}>有未保存的修改</span>}
          {info?.builtin && info.modified && (
            <button type="button" className="btn tr" onClick={() => void reset()}>
              <RotateCcw />
              恢复内置
            </button>
          )}
          {editing !== bot.template && info && fresh.current !== editing && (
            <button type="button" className="btn tr" disabled={busy} onClick={() => void useTemplate()}>
              <Check />
              用于本机器人
            </button>
          )}
          <button type="button" className="btn btn-pri tr" disabled={!dirty} onClick={() => void save()}>
            <Save />
            保存模板
          </button>
        </div>
      </div>
    </div>
  );
}
