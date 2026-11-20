// 机器人设置：名称、引擎、超时、并发、App ID / Secret、禁止清单追加、删除。
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, Eye, EyeOff, Lock, Save, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { deleteBot, errText, saveBot, startBot, stopBot, type Bot, type BotView, type TriggerMode } from "../../api";
import { useToast } from "../common";
import { NumberInput, Segmented } from "../ui";

export const TRIGGER_MODES: [TriggerMode, string][] = [
  ["mention", "@ 机器人"],
  ["pr_link", "群里发 PR 链接自动触发"],
  ["both", "两者都可"],
];

export default function ConfigTab({
  view,
  onChanged,
  onDeleted,
}: {
  view: BotView;
  onChanged: () => Promise<void> | void;
  onDeleted: () => void;
}) {
  const toast = useToast();
  const [form, setForm] = useState<Bot>(view.bot);
  const [deny, setDeny] = useState(view.bot.extra_deny.join("\n"));
  const [secret, setSecret] = useState("");
  const [showSecret, setShowSecret] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // 切换机器人时重置表单
  useEffect(() => {
    setForm(view.bot);
    setDeny(view.bot.extra_deny.join("\n"));
    setSecret("");
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view.bot.id]);

  const set = <K extends keyof Bot>(k: K, v: Bot[K]) => setForm((f) => ({ ...f, [k]: v }));

  const appIdChanged = form.app_id.trim() !== view.bot.app_id;

  const save = async () => {
    setError(null);
    if (appIdChanged && !secret.trim()) {
      setError("更换 App ID 需要同时填写新的 App Secret");
      return;
    }
    setSaving(true);
    try {
      const extra_deny = deny
        .split("\n")
        .map((s) => s.trim())
        .filter(Boolean);
      // 只覆盖本表单负责的字段，其余（随客户端启动、技能、模板、自动接入、群）以最新的机器人配置为准，
      // 避免设置页打开期间在别处做的修改被旧快照冲掉
      const latest = view.bot;
      const saved = await saveBot(
        {
          ...latest,
          name: form.name,
          engine: form.engine,
          timeout_minutes: form.timeout_minutes,
          concurrency: form.concurrency,
          app_id: form.app_id,
          trigger_mode: form.trigger_mode ?? "mention",
          extra_deny,
        },
        secret.trim() || null,
      );
      setForm(saved);
      setSecret("");
      await onChanged();
      if (view.state.kind === "running") {
        toast("已保存，重启机器人后生效", "ok", {
          label: "立即重启",
          onClick: async () => {
            try {
              await stopBot(saved.id);
              await startBot(saved.id);
              await onChanged();
              toast("机器人已重启，新配置已生效");
            } catch (e) {
              toast(`重启失败：${errText(e)}`, "err");
            }
          },
        });
      } else {
        toast("已保存");
      }
    } catch (e) {
      setError(errText(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!window.confirm(`确定删除机器人「${view.bot.name}」吗？只删除本机配置，不会删除飞书应用和记录文档。`)) return;
    try {
      await deleteBot(view.bot.id);
      toast(`已删除「${view.bot.name}」`, "warn");
      onDeleted();
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <div className="scroll flex-1" style={{ padding: "18px 24px" }}>
      {error && (
        <div className="alert-err" role="alert" style={{ marginBottom: 12, maxWidth: 900 }}>
          {error}
        </div>
      )}
      <div className="grid grid-cols-2" style={{ gap: 12, maxWidth: 900 }}>
        <div className="card grid" style={{ padding: 16, gap: 14, alignContent: "start" }}>
          <div style={{ fontWeight: 600 }}>基本信息</div>
          <div>
            <label className="lbl" htmlFor="cfg-name">
              名称
            </label>
            <input id="cfg-name" className="inp" value={form.name} onChange={(e) => set("name", e.target.value)} />
          </div>
          <div>
            <span className="lbl">引擎</span>
            <Segmented
              ariaLabel="引擎"
              size="md"
              value={form.engine}
              onChange={(v) => set("engine", v)}
              options={[
                { value: "claude", label: "Claude Code" },
                { value: "codex", label: "Codex" },
              ]}
            />
          </div>
          <div>
            <span className="lbl">触发方式</span>
            <div className="seg" role="radiogroup" aria-label="触发方式">
              {TRIGGER_MODES.map(([k, t]) => {
                const on = (form.trigger_mode ?? "mention") === k;
                return (
                  <button key={k} type="button" role="radio" aria-checked={on} className={on ? "on" : ""} onClick={() => set("trigger_mode", k)}>
                    {t}
                  </button>
                );
              })}
            </div>
            {(form.trigger_mode ?? "mention") !== "mention" && (
              <div style={{ fontSize: 11, color: "var(--dim)", marginTop: 5 }}>
                群里任何消息带 Gitee / GitHub 的 PR 链接就会审查，同一 PR 10 分钟内不重复审；需要飞书应用开通「获取群组中所有消息」权限
              </div>
            )}
          </div>
          <div className="grid grid-cols-2" style={{ gap: 10 }}>
            <div>
              <label className="lbl" htmlFor="cfg-timeout">
                运行超时（分钟）
              </label>
              <NumberInput id="cfg-timeout" min={1} value={form.timeout_minutes} onChange={(v) => set("timeout_minutes", Number(v))} />
            </div>
            <div>
              <label className="lbl" htmlFor="cfg-conc">
                同时运行数
              </label>
              <NumberInput id="cfg-conc" min={1} value={form.concurrency} onChange={(v) => set("concurrency", Number(v))} />
            </div>
          </div>
        </div>

        <div className="card grid" style={{ padding: 16, gap: 14, alignContent: "start" }}>
          <div style={{ fontWeight: 600 }}>飞书应用</div>
          <div>
            <label className="lbl" htmlFor="cfg-appid">
              App ID
            </label>
            <input id="cfg-appid" className="inp mono" value={form.app_id} onChange={(e) => set("app_id", e.target.value)} />
          </div>
          <div>
            <label className="lbl" htmlFor="cfg-secret">
              App Secret
            </label>
            <div className="flex gap-2">
              <input
                id="cfg-secret"
                className="inp mono"
                type={showSecret ? "text" : "password"}
                autoComplete="off"
                placeholder={appIdChanged ? "更换了 App ID，必须填写新的 App Secret" : "已保存；留空表示不修改"}
                aria-required={appIdChanged}
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
              />
              <button type="button" className="btn tr" aria-label={showSecret ? "隐藏" : "显示"} onClick={() => setShowSecret((v) => !v)}>
                {showSecret ? <EyeOff /> : <Eye />}
              </button>
            </div>
            <div className="flex items-center gap-1" style={{ fontSize: 11, color: "var(--dim)", marginTop: 5 }}>
              <Lock style={{ width: 11, height: 11 }} /> 保存在 Windows 凭据管理器，导出配置时不包含
            </div>
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              className="btn btn-sm tr"
              disabled={!form.app_id}
              onClick={() => void openUrl(`https://open.feishu.cn/app/${form.app_id}`)}
            >
              <ExternalLink />
              飞书开发者后台
            </button>
          </div>
        </div>

        <div className="card" style={{ padding: 16, gridColumn: "1 / -1" }}>
          <div style={{ fontWeight: 600 }}>禁止清单（追加）</div>
          <div style={{ color: "var(--muted)", fontSize: 12, margin: "2px 0 10px" }}>
            在内置禁止规则之外，再禁止引擎执行的命令或工具，一行一条（例如 <span className="mono">Bash(git push:*)</span>）
          </div>
          <textarea
            className="inp mono"
            rows={4}
            aria-label="禁止清单"
            spellCheck={false}
            value={deny}
            onChange={(e) => setDeny(e.target.value)}
          />
        </div>

        <div
          className="card flex items-center"
          style={{ padding: 16, gridColumn: "1 / -1", gap: 12, borderColor: "color-mix(in srgb, var(--danger) 30%, transparent)" }}
        >
          <div className="flex-1">
            <div style={{ fontWeight: 600 }}>删除机器人</div>
            <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>只删除本机配置，不会删除飞书应用和记录文档</div>
          </div>
          <button type="button" className="btn btn-danger tr" onClick={() => void remove()}>
            <Trash2 />
            删除
          </button>
        </div>
      </div>
      <div className="flex justify-end" style={{ marginTop: 14, maxWidth: 900 }}>
        <button type="button" className="btn btn-pri tr" disabled={saving} onClick={() => void save()}>
          <Save />
          保存
        </button>
      </div>
    </div>
  );
}
