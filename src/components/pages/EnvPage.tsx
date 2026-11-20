// 环境检查页：分组清单（运行环境 / 飞书与技能 / 网络），每项状态、版本、说明、操作；全部检查、一键修复全部；安装时展开实时输出。
import {
  CircleCheck,
  ClipboardList,
  CircleX,
  Download,
  ExternalLink,
  FileCode,
  GitBranch,
  Globe,
  Hexagon,
  Info,
  LoaderCircle,
  QrCode,
  RefreshCw,
  ScanSearch,
  Send,
  Shield,
  ShieldAlert,
  ShieldCheck,
  Terminal,
  TriangleAlert,
  Wifi,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { Fragment } from "react";
import type { EnvGroup, EnvItem } from "../../api";
import { useEnvFix } from "../../hooks/useEnvFix";
import { PageHead } from "../common";

export const GROUPS: { id: EnvGroup; label: string }[] = [
  { id: "runtime", label: "运行环境" },
  { id: "feishu", label: "飞书与技能" },
  { id: "network", label: "网络" },
];

const ID_ICON: Record<string, LucideIcon> = {
  claude: Terminal,
  codex: Terminal,
  node: Hexagon,
  python: FileCode,
  git: GitBranch,
  lark_cli: Send,
  lark_login: Send,
  meegle_cli: ClipboardList,
  meegle_login: ClipboardList,
  ocr: ScanSearch,
  net_feishu: Wifi,
  net_anthropic: Globe,
  system_proxy: Shield,
};
export const envIcon = (id: string): LucideIcon => ID_ICON[id] ?? Globe;

const ACTION_ICON: Record<string, LucideIcon> = { install: Download, login: QrCode, fix: Wrench, open_settings: ExternalLink };

export function EnvChip({ item, busy }: { item: EnvItem; busy: boolean }) {
  if (busy || item.status === "checking")
    return (
      <span className="chip c-mut">
        <LoaderCircle className="spin" />
        {busy ? "处理中" : "检查中"}
      </span>
    );
  if (item.status === "ok")
    return (
      <span className="chip c-ok">
        <CircleCheck />
        正常
      </span>
    );
  if (item.status === "error")
    return (
      <span className="chip c-err">
        <CircleX />
        异常
      </span>
    );
  return (
    <span className="chip c-warn">
      <TriangleAlert />
      警告
    </span>
  );
}

/** 需要处理（有修复动作且不是正常）的项。 */
export const fixable = (items: EnvItem[]) => items.filter((e) => e.action && (e.status === "error" || e.status === "warn"));

export function EnvActionButton({ item, busy, disabled, onFix, small = true }: { item: EnvItem; busy: boolean; disabled?: boolean; onFix: () => void; small?: boolean }) {
  if (!item.action || item.status === "ok") return null;
  const Icon = ACTION_ICON[item.action.kind] ?? Wrench;
  const cls = item.action.kind === "fix" ? "btn-warn" : item.status === "error" ? "btn-pri" : "";
  return (
    <button type="button" className={`btn ${small ? "btn-sm" : ""} ${cls} tr`} disabled={busy || disabled} onClick={onFix}>
      {busy ? <LoaderCircle className="spin" /> : <Icon />}
      {item.action.label}
    </button>
  );
}

export default function EnvPage({ env, checking, onRecheck }: { env: EnvItem[]; checking: boolean; onRecheck: () => Promise<void> | void }) {
  const { logs, running, results, batch, fix, fixAll } = useEnvFix();
  const bad = env.filter((e) => e.status === "error").length;
  const warn = env.filter((e) => e.status === "warn").length;
  const ok = env.filter((e) => e.status === "ok").length;
  const todo = fixable(env);

  const runFix = async (id: string) => {
    await fix(id);
    await onRecheck();
  };
  const runAll = async () => {
    await fixAll(todo.map((e) => e.id));
    await onRecheck();
  };

  const summary = bad ? (
    <div
      className="card flex items-center"
      style={{
        padding: "14px 16px",
        gap: 12,
        borderColor: "color-mix(in srgb, var(--danger) 40%, transparent)",
        background: "linear-gradient(90deg, color-mix(in srgb, var(--danger) 8%, transparent), transparent 60%)",
      }}
    >
      <ShieldAlert style={{ width: 22, height: 22, color: "var(--danger-fg)" }} />
      <div className="flex-1">
        <div style={{ fontWeight: 600 }} data-testid="env-summary">
          {bad} 项异常{warn ? `、${warn} 项警告` : ""} · 机器人暂时无法启动
        </div>
        <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>正在运行的机器人不受影响，但重启后会起不来。建议先「一键修复全部」。</div>
      </div>
      <div className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>
        {ok}/{env.length} 通过
      </div>
    </div>
  ) : (
    <div className="card glow-ok flex items-center" style={{ padding: "14px 16px", gap: 12, borderColor: "color-mix(in srgb, var(--success) 40%, transparent)" }}>
      <ShieldCheck style={{ width: 22, height: 22, color: "var(--success-fg)" }} />
      <div className="flex-1">
        <div style={{ fontWeight: 600 }} data-testid="env-summary">
          {env.length === 0 ? "还没有检查结果" : warn ? `核心依赖全部通过，还有 ${warn} 项警告` : "环境全部通过"}
        </div>
        <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>{env.length === 0 ? "点右上角「全部检查」" : "机器人可以正常启动。"}</div>
      </div>
      <div className="mono" style={{ color: "var(--muted)", fontSize: 12 }}>
        {ok}/{env.length} 通过
      </div>
    </div>
  );

  return (
    <>
      <PageHead
        title="环境检查"
        sub="机器人不稳定大多是这里出问题：CLI 不在 PATH、登录过期、代理设错"
        right={
          <>
            <button type="button" className="btn tr" disabled={checking || batch} onClick={() => void onRecheck()}>
              <RefreshCw className={checking ? "spin" : ""} />
              全部检查
            </button>
            <button type="button" className="btn btn-pri tr" disabled={!todo.length || batch || running.size > 0} onClick={() => void runAll()}>
              <Wrench />
              一键修复全部
            </button>
          </>
        }
      />
      <div className="scroll flex-1" style={{ padding: "16px 24px" }}>
        {summary}
        {GROUPS.map((g) => {
          const items = env.filter((e) => e.group === g.id);
          if (!items.length) return null;
          return (
            <Fragment key={g.id}>
              <div className="grp-t" style={{ margin: "18px 2px 8px" }}>
                {g.label}
              </div>
              <div className="card">
                <div className="t-scroll">
                <table className="t">
                  <thead>
                    <tr>
                      <th style={{ width: 220 }}>检查项</th>
                      <th style={{ width: 90 }}>状态</th>
                      <th style={{ width: 110 }}>版本 / 延迟</th>
                      <th>说明</th>
                      <th style={{ textAlign: "right", width: 190 }}>操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {items.map((e) => {
                      const busy = running.has(e.id);
                      const lines = logs[e.id] ?? [];
                      const res = results[e.id];
                      return (
                        <Fragment key={e.id}>
                          <tr data-testid={`env-row-${e.id}`} style={e.status === "error" ? { background: "color-mix(in srgb, var(--danger) 4%, transparent)" } : undefined}>
                            <td className="nw" style={{ fontWeight: 500 }}>
                              <span className="flex items-center gap-2.5">
                                {(() => {
                                  const Icon = envIcon(e.id);
                                  return <Icon style={{ width: 16, height: 16, color: "var(--muted)" }} />;
                                })()}
                                {e.label}
                              </span>
                            </td>
                            <td>
                              <EnvChip item={e} busy={busy} />
                            </td>
                            <td className="mono nw" style={{ color: e.status === "error" && !e.version ? "var(--danger-fg)" : "var(--muted)" }}>
                              {e.version ?? (e.status === "error" ? "未找到" : "—")}
                            </td>
                            <td style={{ color: "var(--muted)", fontSize: 12, lineHeight: 1.5, whiteSpace: "normal" }}>
                              {e.detail}
                              {res && !busy && (
                                <div style={{ color: res.ok ? "var(--success-fg)" : "var(--danger-fg)", marginTop: 2 }} data-testid={`env-result-${e.id}`}>
                                  {res.ok ? "已完成" : "未成功"}：{res.message}
                                </div>
                              )}
                            </td>
                            <td style={{ textAlign: "right" }}>
                              <div className="flex justify-end">
                                <EnvActionButton item={e} busy={busy} disabled={batch && !busy} onFix={() => void runFix(e.id)} />
                              </div>
                            </td>
                          </tr>
                          {(busy || lines.length > 0) && (
                            <tr>
                              <td colSpan={5} style={{ padding: "0 14px 10px" }}>
                                <pre
                                  className="term scroll raw"
                                  data-testid={`env-log-${e.id}`}
                                  style={{ maxHeight: 180, margin: 0, padding: "8px 12px", color: "var(--term-fg)" }}
                                >
                                  {lines.length ? lines.join("\n") : "等待输出…"}
                                </pre>
                              </td>
                            </tr>
                          )}
                        </Fragment>
                      );
                    })}
                  </tbody>
                </table>
                </div>
              </div>
            </Fragment>
          );
        })}
        <div className="flex items-center gap-2" style={{ color: "var(--dim)", fontSize: 12, marginTop: 12 }}>
          <Info style={{ width: 13, height: 13 }} />
          安装使用 winget / npm，需要管理员权限时会弹出 Windows 确认框；登录类会打开新的终端窗口。不要设置系统级 HTTPS_PROXY，会让飞书长连接走代理。
        </div>
      </div>
    </>
  );
}
