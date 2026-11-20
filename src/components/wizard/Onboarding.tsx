// 首次启动引导：欢迎 → 环境检查（可一键修复，必需项未过不能下一步）→ 检测旧版（可从旧版导入，停旧 bridge 前确认）
// → 创建机器人（进入向导）或导入同事配置 → 完成。config 里没有机器人时自动进入，设置页可重新打开。
import { ArrowLeft, ArrowRight, CircleCheck, CircleX, History, LoaderCircle, QrCode, TriangleAlert, Upload, Wrench } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { detectLegacy, errText, migrateLegacy, type EnvItem, type EnvRequirements, type LegacyInfo } from "../../api";
import { onboardingBlockers } from "../../envRules";
import { useEnvFix } from "../../hooks/useEnvFix";
import BrandLogo from "../BrandLogo";
import ImportConfig from "../ImportConfig";
import { EnvActionButton, envIcon, fixable } from "../pages/EnvPage";
import { Checkbox, Modal } from "../ui";
import Steps from "./Steps";

const NAMES = ["欢迎", "环境检查", "从旧版导入", "创建机器人", "完成"];

export default function Onboarding({
  env,
  requirements = null,
  checking,
  onRecheck,
  onClose,
  onCreateBot,
  onChanged,
}: {
  env: EnvItem[];
  /** 后端 env_requirements；没有时按同样的基础项 / 引擎项兜底 */
  requirements?: EnvRequirements | null;
  checking: boolean;
  onRecheck: () => Promise<void> | void;
  onClose: () => void;
  /** 关闭引导、打开新增机器人向导 */
  onCreateBot: () => void;
  /** 导入了机器人 / 配置后刷新列表 */
  onChanged: () => Promise<void> | void;
}) {
  const [step, setStep] = useState(1);
  const { logs, running, results, batch, fix, fixAll } = useEnvFix();
  const [legacy, setLegacy] = useState<LegacyInfo | null>(null);
  const [legacyError, setLegacyError] = useState<string | null>(null);
  const [stopBridge, setStopBridge] = useState(true);
  const [migrating, setMigrating] = useState(false);
  const [migrated, setMigrated] = useState<{ warnings: string[] } | null>(null);
  const [imported, setImported] = useState(false);

  useEffect(() => {
    if (step === 2) void onRecheck();
    if (step === 3 && !legacy) {
      detectLegacy()
        .then((l) => {
          setLegacy(l);
          setStopBridge(l.bridge_running || l.task_enabled);
        })
        .catch((e) => setLegacyError(errText(e)));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step]);

  // 只拦基础项 + 「至少一个引擎可用」（只装 Claude Code 不装 Codex 也能继续）
  const errors = onboardingBlockers(env, requirements);
  const engineIds = requirements?.engines ?? ["claude", "codex"];
  const engineBlocked = errors.some((e) => engineIds.includes(e.id));
  const todo = fixable(env);

  const runFix = async (id: string) => {
    await fix(id);
    await onRecheck();
  };

  const doMigrate = async () => {
    if (stopBridge && !window.confirm("将结束正在运行的旧版 bridge 进程，并尝试禁用它的计划任务。旧版机器人会停止回复，确定继续吗？")) return;
    setLegacyError(null);
    setMigrating(true);
    try {
      const r = await migrateLegacy(stopBridge);
      setMigrated({ warnings: r.warnings });
      await onChanged();
    } catch (e) {
      setLegacyError(errText(e));
    } finally {
      setMigrating(false);
    }
  };

  let body: ReactNode = null;
  if (step === 1)
    body = (
      <div className="grid items-center" style={{ gridTemplateColumns: "1fr 1fr", gap: 28 }}>
        <div>
          <div style={{ width: 52, height: 52, borderRadius: 14, boxShadow: "0 0 30px color-mix(in srgb, var(--accent) 25%, transparent)" }}>
            <BrandLogo size={52} />
          </div>
          <div style={{ fontSize: 22, fontWeight: 700, marginTop: 16 }}>欢迎使用 Beetle</div>
          <div style={{ color: "var(--muted)", lineHeight: 1.8, marginTop: 8 }}>
            在飞书群里 @ 机器人，它会在你这台电脑上用 Claude Code / Codex 读群聊、同步最新 release 代码、只读查关联的数据源，把结论回复到群里。
          </div>
          <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 12 }}>大约 3 分钟完成设置：检查环境 → 导入旧版（如有）→ 创建第一个机器人</div>
        </div>
        <div className="term" style={{ padding: "12px 0" }}>
          {[
            ["14:02:03", "收到 @ 消息", "订单状态不刷新"],
            ["14:02:09", "读群聊", "命中上下文 9 条"],
            ["14:02:31", "同步代码", "release/2026.11.18"],
            ["14:03:12", "查库 ×4", "数据源只读"],
            ["14:06:51", "写记录文档", "问题记录 #23"],
            ["14:08:15", "回复群里", "已发送"],
          ].map(([t, a, b]) => (
            <div key={t} className="tl">
              <span className="ts">{t}</span>
              <span className="ic">
                <CircleCheck style={{ color: "var(--success-fg)" }} />
              </span>
              <span>
                <span style={{ fontWeight: 500 }}>{a}</span> <span className="sub">{b}</span>
              </span>
              <span />
            </div>
          ))}
        </div>
      </div>
    );
  if (step === 2)
    body = (
      <>
        <div className="flex items-center" style={{ marginBottom: 12, gap: 8 }}>
          <span className="flex-1" style={{ color: "var(--muted)" }}>
            正在检查这台电脑的运行环境，缺的可以一键装上（winget / npm）。
          </span>
          {todo.length > 0 && (
            <button type="button" className="btn btn-sm tr" disabled={batch || running.size > 0} onClick={() => void fixAll(todo.map((e) => e.id)).then(() => onRecheck())}>
              <Wrench />
              一键修复全部
            </button>
          )}
        </div>
        <div className="card" data-testid="onboarding-env">
          {checking && env.length === 0 && (
            <div className="flex items-center gap-2" style={{ padding: 14, color: "var(--muted)" }}>
              <LoaderCircle className="spin" style={{ width: 14, height: 14 }} />
              检查中…
            </div>
          )}
          {env.map((e) => {
            const busy = running.has(e.id);
            const Icon = envIcon(e.id);
            const lines = logs[e.id] ?? [];
            return (
              <div key={e.id} style={{ borderBottom: "1px solid var(--line)" }}>
                <div className="flex items-center" style={{ gap: 12, padding: "9px 14px" }}>
                  {busy ? (
                    <LoaderCircle className="spin" style={{ width: 16, height: 16, color: "var(--warning-fg)" }} />
                  ) : e.status === "ok" ? (
                    <CircleCheck style={{ width: 16, height: 16, color: "var(--success-fg)" }} />
                  ) : e.status === "error" ? (
                    <CircleX style={{ width: 16, height: 16, color: "var(--danger-fg)" }} />
                  ) : (
                    <TriangleAlert style={{ width: 16, height: 16, color: "var(--warning-fg)" }} />
                  )}
                  <Icon style={{ width: 14, height: 14, color: "var(--muted)" }} />
                  <span style={{ width: 170, fontWeight: 500 }}>{e.label}</span>
                  <span className="mono" style={{ width: 80, color: "var(--muted)" }}>
                    {e.version ?? "—"}
                  </span>
                  <span className="min-w-0 flex-1 truncate" style={{ color: "var(--muted)", fontSize: 12 }}>
                    {results[e.id] && !busy ? results[e.id].message : e.detail}
                  </span>
                  <EnvActionButton item={e} busy={busy} disabled={batch && !busy} onFix={() => void runFix(e.id)} />
                </div>
                {(busy || lines.length > 0) && (
                  <pre className="raw mono" style={{ margin: "0 14px 8px", maxHeight: 120, overflow: "auto", fontSize: 11, background: "var(--term-bg)", borderRadius: 7 }}>
                    {lines.length ? lines.join("\n") : "等待输出…"}
                  </pre>
                )}
              </div>
            );
          })}
        </div>
        {errors.length > 0 && (
          <div className="flex items-center gap-1.5" style={{ marginTop: 10, color: "var(--danger-fg)", fontSize: 12 }}>
            <TriangleAlert style={{ width: 13, height: 13 }} />
            还有 {errors.length} 项必需环境未通过（{errors.map((e) => e.label).join("、")}），修好才能继续
            {engineBlocked && "（引擎装好其中一个即可）"}
          </div>
        )}
      </>
    );
  if (step === 3)
    body = (
      <>
        {legacyError && (
          <div className="alert-err" role="alert" style={{ fontSize: 12, marginBottom: 12 }}>
            {legacyError}
          </div>
        )}
        {!legacy && !legacyError && (
          <div className="flex items-center gap-2" style={{ color: "var(--muted)" }}>
            <LoaderCircle className="spin" style={{ width: 14, height: 14 }} />
            正在检测旧版 bug-bot…
          </div>
        )}
        {legacy && !legacy.found && (
          <div className="card flex items-center" style={{ padding: 16, gap: 12 }} data-testid="legacy-none">
            <History style={{ width: 20, height: 20, color: "var(--muted)" }} />
            <div>
              <div style={{ fontWeight: 600 }}>没有发现旧版 bug-bot</div>
              <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>直接下一步创建机器人即可</div>
            </div>
          </div>
        )}
        {legacy?.found && (
          <div className="grid" style={{ gap: 12 }}>
            <div className="card" style={{ padding: 16 }} data-testid="legacy-found">
              <div className="flex items-center" style={{ gap: 10 }}>
                <History style={{ width: 20, height: 20, color: "var(--accent-text)" }} />
                <div className="flex-1" style={{ fontWeight: 600 }}>
                  发现旧版 bug-bot（lark-channel-bridge）
                </div>
                {legacy.bridge_running && <span className="chip c-warn">旧 bridge 正在运行</span>}
              </div>
              <div className="grid" style={{ gridTemplateColumns: "90px 1fr", gap: "6px 10px", marginTop: 12, fontSize: 12 }}>
                <span style={{ color: "var(--muted)" }}>飞书应用</span>
                <span className="mono">{legacy.app_id ?? "—"}</span>
                <span style={{ color: "var(--muted)" }}>群</span>
                <span>{legacy.groups.map((g) => g.name).join("、") || "无"}</span>
                <span style={{ color: "var(--muted)" }}>项目</span>
                <span className="mono">{legacy.projects.join("、") || "无"}</span>
              </div>
            </div>
            <Checkbox
              checked={stopBridge}
              onChange={setStopBridge}
              label="导入后停用旧版"
              description="结束旧 bridge 进程并禁用计划任务（同一个飞书应用只能连一处）"
            />
            {migrated ? (
              <div className="card" style={{ padding: 12 }} data-testid="legacy-done">
                <div className="flex items-center gap-2" style={{ color: "var(--success-fg)", fontWeight: 600 }}>
                  <CircleCheck style={{ width: 15, height: 15 }} />
                  已从旧版导入机器人、群、项目和库
                </div>
                {migrated.warnings.map((w) => (
                  <div key={w} className="flex items-start gap-1.5" style={{ color: "var(--warning-fg)", fontSize: 12, marginTop: 6 }}>
                    <TriangleAlert style={{ width: 13, height: 13, marginTop: 2 }} />
                    {w}
                  </div>
                ))}
              </div>
            ) : (
              <div>
                <button type="button" className="btn btn-pri tr" disabled={migrating} onClick={() => void doMigrate()}>
                  {migrating ? <LoaderCircle className="spin" /> : <Upload />}
                  从旧版导入
                </button>
              </div>
            )}
          </div>
        )}
      </>
    );
  if (step === 4)
    body = (
      <div style={{ padding: "4px 0" }}>
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 18, fontWeight: 700 }}>{migrated ? "已有一个从旧版导入的机器人" : "最后一步：创建第一个机器人"}</div>
          <div style={{ color: "var(--muted)", marginTop: 6 }}>可以扫码新建飞书应用，也可以导入同事分享的配置（项目、技能、模板，不含密钥）</div>
        </div>
        <div className="grid grid-cols-2" style={{ gap: 12, marginTop: 20 }}>
          <button type="button" className="opt tr on" style={{ padding: 16 }} onClick={onCreateBot}>
            <QrCode style={{ width: 22, height: 22, color: "var(--accent-text)" }} />
            <div style={{ fontWeight: 600, marginTop: 8 }}>新建机器人</div>
            <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 4 }}>扫码创建飞书应用 → 选模板和技能 → 绑定群</div>
          </button>
          <div className="opt" style={{ padding: 16 }}>
            <Upload style={{ width: 22, height: 22, color: "var(--muted)" }} />
            <div style={{ fontWeight: 600, marginTop: 8 }}>导入同事配置</div>
            <div style={{ color: "var(--muted)", fontSize: 12, margin: "4px 0 10px" }}>选择 beetle-config.json</div>
            <ImportConfig
              onImported={() => {
                setImported(true);
                void onChanged();
              }}
            />
          </div>
        </div>
      </div>
    );
  if (step === 5)
    body = (
      <div style={{ textAlign: "center", padding: "30px 0" }} data-testid="onboarding-done">
        <div
          className="grid place-items-center"
          style={{ width: 60, height: 60, borderRadius: 16, margin: "0 auto", background: "var(--success-soft)", boxShadow: "0 0 30px color-mix(in srgb, var(--success) 20%, transparent)" }}
        >
          <CircleCheck style={{ width: 30, height: 30, color: "var(--success-fg)" }} />
        </div>
        <div style={{ fontSize: 18, fontWeight: 700, marginTop: 14 }}>设置完成</div>
        <div style={{ color: "var(--muted)", marginTop: 6 }}>
          {migrated ? "旧版配置已导入，" : ""}
          {imported ? "同事配置已导入，" : ""}
          之后可以在左下角「新增机器人」，或在「设置」里重新打开本引导。
        </div>
      </div>
    );

  const canNext = step !== 2 || (errors.length === 0 && !checking);

  return (
    <Modal
      ariaLabel="首次启动引导"
      strongOverlay
      width={780}
      bodyStyle={{ padding: "24px 26px", minHeight: 330 }}
      header={
        <div className="ui-modal-h" style={{ gap: 16, paddingRight: 16 }}>
          <span style={{ fontSize: 12, color: "var(--muted)" }}>首次启动</span>
          <div className="flex-1">
            <Steps cur={step} names={NAMES} />
          </div>
          <button type="button" className="btn btn-ghost btn-sm tr" onClick={onClose}>
            跳过
          </button>
        </div>
      }
      footer={
        <div className="flex flex-1 justify-between">
          {step > 1 && step < 5 ? (
            <button type="button" className="btn tr" onClick={() => setStep(step - 1)}>
              <ArrowLeft />
              上一步
            </button>
          ) : (
            <span />
          )}
          {step < 5 ? (
            <button type="button" className="btn btn-pri tr" disabled={!canNext} onClick={() => setStep(step + 1)}>
              {step === 1 ? "开始设置" : step === 4 && !migrated && !imported ? "跳过，稍后创建" : "下一步"} <ArrowRight />
            </button>
          ) : (
            <button type="button" className="btn btn-pri tr" onClick={onClose}>
              完成
            </button>
          )}
        </div>
      }
    >
      {body}
    </Modal>
  );
}
