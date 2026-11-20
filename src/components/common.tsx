// 通用小组件：状态标签、开关、页头、Toast、外观控件。样式类见 src/theme/components.css。
import {
  Ban,
  Check,
  CircleAlert,
  CircleCheck,
  CirclePause,
  CircleX,
  Clock,
  LoaderCircle,
  Monitor,
  Moon,
  Radio,
  Sun,
  TimerOff,
  type LucideIcon,
} from "lucide-react";
import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import type { BotState, RunStatus } from "../api";
import { ACCENTS, MODES, useAppearance } from "../theme/appearance";

// ───────────── 状态 ─────────────
export type BotKind = BotState["kind"] | "stopping";

export const BOT_META: Record<BotKind, { text: string; dot: string; chip: string; Icon: LucideIcon; spin?: boolean }> = {
  running: { text: "运行中", dot: "st-running pulse", chip: "c-ok", Icon: Radio },
  stopped: { text: "已停止", dot: "st-stopped", chip: "c-mut", Icon: CirclePause },
  connecting: { text: "连接中", dot: "st-connecting blink", chip: "c-warn", Icon: LoaderCircle, spin: true },
  stopping: { text: "停止中", dot: "st-connecting blink", chip: "c-warn", Icon: LoaderCircle, spin: true },
  error: { text: "异常", dot: "st-error", chip: "c-err", Icon: CircleAlert },
};

export function BotChip({ kind }: { kind: BotKind }) {
  const m = BOT_META[kind];
  return (
    <span className={`chip ${m.chip}`}>
      <m.Icon className={m.spin ? "spin" : ""} />
      {m.text}
    </span>
  );
}

export const RUN_META: Record<RunStatus, { text: string; chip: string; Icon: LucideIcon; spin?: boolean }> = {
  queued: { text: "排队中", chip: "c-mut", Icon: Clock },
  running: { text: "运行中", chip: "c-ok", Icon: LoaderCircle, spin: true },
  replied: { text: "已回复", chip: "c-ok", Icon: CircleCheck },
  failed: { text: "失败", chip: "c-err", Icon: CircleX },
  timeout: { text: "超时", chip: "c-warn", Icon: TimerOff },
  cancelled: { text: "已取消", chip: "c-mut", Icon: Ban },
};

export function RunChip({ status }: { status: RunStatus }) {
  const m = RUN_META[status];
  return (
    <span className={`chip ${m.chip}`} data-testid="run-chip">
      <m.Icon className={m.spin ? "spin" : ""} />
      {m.text}
    </span>
  );
}

// ───────────── 控件 ─────────────
export function Switch({
  on,
  label,
  onChange,
  disabled,
  showText = true,
}: {
  on: boolean;
  label: string;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  showText?: boolean;
}) {
  return (
    <>
      <button
        type="button"
        className={`sw ${on ? "on" : ""}`}
        role="switch"
        aria-checked={on}
        aria-label={label}
        disabled={disabled}
        onClick={() => onChange(!on)}
      />
      {showText && <span style={{ width: 14, color: "var(--fg)" }}>{on ? "开" : "关"}</span>}
    </>
  );
}

export function PageHead({ title, sub, right }: { title: string; sub: string; right?: ReactNode }) {
  return (
    <div
      className="flex flex-none items-center justify-between"
      style={{ height: 64, padding: "0 24px", borderBottom: "1px solid var(--line)" }}
    >
      <div>
        <div style={{ fontSize: 16, fontWeight: 600 }}>{title}</div>
        <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>{sub}</div>
      </div>
      <div className="flex items-center gap-2">{right}</div>
    </div>
  );
}

export function SettingRow({ title, desc, children, last }: { title: string; desc: ReactNode; children: ReactNode; last?: boolean }) {
  return (
    <div className="flex items-center gap-4" style={{ padding: "14px 16px", borderBottom: last ? 0 : "1px solid var(--line)" }}>
      <div className="flex-1">
        <div style={{ fontWeight: 500 }}>{title}</div>
        <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>{desc}</div>
      </div>
      {children}
    </div>
  );
}

// ───────────── 外观控件（标题栏浮层与设置页共用） ─────────────
const MODE_ICON = { dark: Moon, light: Sun, system: Monitor } as const;

export function ModeSeg() {
  const { appearance, setAppearance } = useAppearance();
  return (
    <div className="seg" role="radiogroup" aria-label="外观模式">
      {MODES.map((m) => {
        const Icon = MODE_ICON[m.id];
        const on = appearance.mode === m.id;
        return (
          <button
            key={m.id}
            type="button"
            className={`tr ${on ? "on" : ""}`}
            role="radio"
            aria-checked={on}
            data-mode-opt={m.id}
            onClick={() => setAppearance({ mode: m.id })}
          >
            <Icon />
            {m.label}
          </button>
        );
      })}
    </div>
  );
}

export function AccentSwatches() {
  const { appearance, setAppearance } = useAppearance();
  return (
    <div className="swatches" role="radiogroup" aria-label="主题色">
      {ACCENTS.map((a) => {
        const on = appearance.accent === a.id;
        return (
          <button
            key={a.id}
            type="button"
            className={`swatch tr ${on ? "on" : ""}`}
            role="radio"
            aria-checked={on}
            aria-label={`主题色：${a.label}`}
            title={a.label}
            data-accent-opt={a.id}
            onClick={() => setAppearance({ accent: a.id })}
          >
            <Check />
          </button>
        );
      })}
    </div>
  );
}

// ───────────── Toast ─────────────
type ToastKind = "ok" | "warn" | "err";
/** 提示里的操作按钮（如「立即重启」）：点了执行并关闭提示。带按钮的提示停留更久。 */
export interface ToastAction {
  label: string;
  onClick: () => void | Promise<void>;
}
interface ToastMsg {
  text: string;
  kind: ToastKind;
  id: number;
  action?: ToastAction;
}
const ToastContext = createContext<(text: string, kind?: ToastKind, action?: ToastAction) => void>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [msg, setMsg] = useState<ToastMsg | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const show = useCallback((text: string, kind: ToastKind = "ok", action?: ToastAction) => {
    clearTimeout(timer.current);
    setMsg({ text, kind, id: Date.now(), action });
    timer.current = setTimeout(() => setMsg(null), action ? 10000 : kind === "err" ? 5000 : 2600);
  }, []);
  const color = msg?.kind === "err" ? "var(--danger-fg)" : msg?.kind === "warn" ? "var(--warning-fg)" : "var(--success-fg)";
  const Icon = msg?.kind === "err" ? CircleX : msg?.kind === "warn" ? CircleAlert : CircleCheck;
  return (
    <ToastContext.Provider value={show}>
      {children}
      {msg && (
        <div className="toast" role="status" key={msg.id}>
          <Icon style={{ width: 16, height: 16, color }} />
          <span>{msg.text}</span>
          {msg.action && (
            <button
              type="button"
              className="btn btn-pri btn-sm tr"
              onClick={() => {
                const a = msg.action;
                clearTimeout(timer.current);
                setMsg(null);
                void a?.onClick();
              }}
            >
              {msg.action.label}
            </button>
          )}
        </div>
      )}
    </ToastContext.Provider>
  );
}

export const useToast = () => useContext(ToastContext);
