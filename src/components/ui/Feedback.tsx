// 提示与状态：Tooltip（悬停 / 聚焦显示，挂到 body 不被裁掉）、Truncate（单行省略，被截断时悬停显示全文）、
// EmptyState（空状态：图标 + 标题 + 说明 + 操作）、LoadingState（加载中）。
import { CircleHelp, Inbox, LoaderCircle, type LucideIcon } from "lucide-react";
import { useEffect, useId, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";

interface TipPos {
  x: number;
  y: number;
  side: "top" | "bottom";
}

export function Tooltip({
  content,
  children,
  side = "top",
  delay = 300,
  onlyWhenTruncated,
  className,
  style,
  as: Tag = "span",
}: {
  content: ReactNode;
  children: ReactNode;
  side?: "top" | "bottom";
  delay?: number;
  /** 只有文字被截断（scrollWidth > clientWidth）时才显示 */
  onlyWhenTruncated?: boolean;
  className?: string;
  style?: CSSProperties;
  as?: "span" | "div";
}) {
  const ref = useRef<HTMLElement>(null);
  const [pos, setPos] = useState<TipPos | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const id = useId();

  const show = () => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      const el = ref.current;
      if (!el || content == null || content === "") return;
      if (onlyWhenTruncated && el.scrollWidth <= el.clientWidth) return;
      const r = el.getBoundingClientRect();
      const flip = side === "top" && r.top < 40;
      const s = flip ? "bottom" : side;
      setPos({ x: r.left + r.width / 2, y: s === "top" ? r.top - 6 : r.bottom + 6, side: s });
    }, delay);
  };
  const hide = () => {
    clearTimeout(timer.current);
    setPos(null);
  };
  useEffect(() => () => clearTimeout(timer.current), []);

  return (
    <>
      <Tag
        ref={ref as never}
        className={className}
        style={style}
        aria-describedby={pos ? id : undefined}
        onMouseEnter={show}
        onMouseLeave={hide}
        onFocus={show}
        onBlur={hide}
      >
        {children}
      </Tag>
      {pos &&
        createPortal(
          <div
            id={id}
            role="tooltip"
            className="ui-tip"
            style={{
              left: Math.max(12, Math.min(pos.x, (window.innerWidth || 1024) - 12)),
              top: pos.y,
              transform: pos.side === "top" ? "translate(-50%, -100%)" : "translate(-50%, 0)",
            }}
          >
            {content}
          </div>,
          document.body,
        )}
    </>
  );
}

/** 文字旁的小问号：悬停 / 聚焦显示说明。 */
export function HelpTip({ text, label = "说明" }: { text: string; label?: string }) {
  return (
    <Tooltip content={text} delay={150} className="help-tip">
      <span tabIndex={0} role="img" aria-label={`${label}：${text}`} className="help-tip-ic">
        <CircleHelp />
      </span>
    </Tooltip>
  );
}

/** 单行省略；被截断时悬停显示全文。 */
export function Truncate({ text, className, style, mono }: { text: string; className?: string; style?: CSSProperties; mono?: boolean }) {
  return (
    <Tooltip content={text} onlyWhenTruncated className={`block min-w-0 truncate ${mono ? "mono" : ""} ${className ?? ""}`} style={style}>
      {text}
    </Tooltip>
  );
}

export function EmptyState({
  icon: Icon = Inbox,
  title,
  desc,
  action,
  compact,
  testId,
  style,
}: {
  icon?: LucideIcon;
  title: ReactNode;
  desc?: ReactNode;
  action?: ReactNode;
  compact?: boolean;
  testId?: string;
  style?: CSSProperties;
}) {
  return (
    <div className={`ui-empty ${compact ? "compact" : ""}`} data-testid={testId} style={style}>
      <div className="ui-empty-ic">
        <Icon />
      </div>
      <div className="ui-empty-t">{title}</div>
      {desc != null && <div className="ui-empty-d">{desc}</div>}
      {action != null && <div className="ui-empty-a">{action}</div>}
    </div>
  );
}

export function LoadingState({ text = "读取中…", style }: { text?: string; style?: CSSProperties }) {
  return (
    <div className="ui-loading" style={style}>
      <LoaderCircle className="spin" />
      {text}
    </div>
  );
}
