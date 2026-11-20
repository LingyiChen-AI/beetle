// 统一的弹窗外壳：遮罩 + 标题栏（图标、标题、副标题、关闭）+ 可滚动内容 + 底部按钮区。
// Esc 关闭（只响应最上层弹窗），Tab 焦点锁在弹窗内，打开时聚焦第一个输入框（没有就聚焦弹窗本身），关闭后焦点还给打开前的元素。
// 进出场动画用 components.css 的 fade / pop，系统开启「减少动态效果」时全局关掉。
import { X, type LucideIcon } from "lucide-react";
import { useEffect, useRef, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";

/** 打开中的弹窗栈：Esc 只给最上层。 */
const stack: symbol[] = [];

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusables(root: HTMLElement | null): HTMLElement[] {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => !el.hasAttribute("data-focus-skip"));
}

export function Modal({
  title,
  ariaLabel,
  icon: Icon,
  subtitle,
  header,
  headerExtra,
  children,
  footer,
  onClose,
  closeDisabled,
  dismissOnOverlay = true,
  width = 520,
  height,
  strongOverlay,
  bodyStyle,
  bodyClassName,
  testId,
}: {
  title?: ReactNode;
  /** 标题不是纯文字时必须给 */
  ariaLabel?: string;
  icon?: LucideIcon;
  subtitle?: ReactNode;
  /** 完全自定义的标题栏（替换默认的） */
  header?: ReactNode;
  /** 默认标题栏里、关闭按钮左边的内容 */
  headerExtra?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  /** 不传 = 没有关闭按钮、Esc 和点遮罩都不关 */
  onClose?: () => void;
  /** 操作进行中：暂时不允许关闭 */
  closeDisabled?: boolean;
  dismissOnOverlay?: boolean;
  width?: number | string;
  height?: number | string;
  strongOverlay?: boolean;
  bodyStyle?: CSSProperties;
  bodyClassName?: string;
  testId?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const downOnOverlay = useRef(false);
  const closeRef = useRef<{ onClose?: () => void; closeDisabled?: boolean }>({});
  closeRef.current = { onClose, closeDisabled };

  useEffect(() => {
    const me = Symbol("modal");
    stack.push(me);
    const prev = document.activeElement as HTMLElement | null;
    const root = ref.current;
    const auto = root?.querySelector<HTMLElement>("[data-autofocus]") ?? root?.querySelector<HTMLElement>("input:not([disabled]), textarea:not([disabled])");
    (auto ?? root)?.focus({ preventScroll: true });
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || stack[stack.length - 1] !== me) return;
      const { onClose: close, closeDisabled: locked } = closeRef.current;
      if (close && !locked) {
        e.preventDefault();
        close();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      stack.splice(stack.indexOf(me), 1);
      if (prev && document.contains(prev)) prev.focus({ preventScroll: true });
    };
  }, []);

  const trap = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Tab") return;
    const els = focusables(ref.current);
    if (!els.length) {
      e.preventDefault();
      return;
    }
    const first = els[0];
    const last = els[els.length - 1];
    const cur = document.activeElement;
    if (e.shiftKey && (cur === first || cur === ref.current)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && cur === last) {
      e.preventDefault();
      first.focus();
    }
  };

  const label = ariaLabel ?? (typeof title === "string" ? title : undefined);

  return (
    <div
      className={`ov ${strongOverlay ? "ov-strong" : ""}`}
      onMouseDown={(e) => (downOnOverlay.current = e.target === e.currentTarget)}
      onClick={(e) => {
        if (e.target === e.currentTarget && downOnOverlay.current && dismissOnOverlay && onClose && !closeDisabled) onClose();
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
        className="modal ui-modal flex flex-col"
        style={{ width, height, maxHeight: "calc(100vh - 80px)" }}
        data-testid={testId}
        onKeyDown={trap}
      >
        {header ??
          (title != null && (
            <div className="ui-modal-h">
              {Icon && (
                <span className="ui-modal-ic">
                  <Icon />
                </span>
              )}
              <div className="min-w-0 flex-1">
                <div className="ui-modal-t">{title}</div>
                {subtitle != null && <div className="ui-modal-sub">{subtitle}</div>}
              </div>
              {headerExtra}
              {onClose && (
                <button type="button" className="btn btn-ghost btn-sm tr" aria-label="关闭" disabled={closeDisabled} onClick={onClose}>
                  <X />
                </button>
              )}
            </div>
          ))}
        <div className={`scroll min-h-0 flex-1 ${bodyClassName ?? ""}`} style={{ padding: "16px 20px", ...bodyStyle }}>
          {children}
        </div>
        {footer != null && <div className="ui-modal-f">{footer}</div>}
      </div>
    </div>
  );
}
