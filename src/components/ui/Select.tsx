// 自绘下拉：触发按钮（combobox）+ 浮层列表（listbox，挂到 body）。
// 键盘：↑↓ / Home / End 移动，Enter / 空格 选中，Esc / Tab 关闭；点外部关闭。选中项打勾，选项可带图标和说明。
import { Check, ChevronDown, type LucideIcon } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { popStyle, useAnchoredPosition, useOutsideClose } from "./popover";

export interface SelectOption<T extends string = string> {
  value: T;
  label: string;
  description?: string;
  icon?: LucideIcon;
  disabled?: boolean;
}

export interface SelectProps<T extends string> {
  value: T | null;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  /** 配合 <label htmlFor> 使用 */
  id?: string;
  ariaLabel?: string;
  placeholder?: string;
  size?: "sm" | "md";
  disabled?: boolean;
  invalid?: boolean;
  /** 选项文字用等宽字体 */
  mono?: boolean;
  className?: string;
  style?: CSSProperties;
  testId?: string;
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  id,
  ariaLabel,
  placeholder = "请选择",
  size = "md",
  disabled,
  invalid,
  mono,
  className,
  style,
  testId,
}: SelectProps<T>) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();
  const pos = useAnchoredPosition(trigger, open);
  const current = options.find((o) => o.value === value) ?? null;
  const close = useCallback(() => setOpen(false), []);
  useOutsideClose([trigger, list], open, close);

  const firstEnabled = (from: number, dir: 1 | -1) => {
    const n = options.length;
    for (let k = 0, i = from; k < n; k++, i = (i + dir + n) % n) if (!options[i]?.disabled) return i;
    return -1;
  };

  const openMenu = () => {
    if (disabled) return;
    const sel = options.findIndex((o) => o.value === value);
    setActive(sel >= 0 ? sel : firstEnabled(0, 1));
    setOpen(true);
  };

  const choose = (o: SelectOption<T> | undefined) => {
    if (!o || o.disabled) return;
    setOpen(false);
    if (o.value !== value) onChange(o.value);
    trigger.current?.focus();
  };

  const move = (dir: 1 | -1) => {
    const n = options.length;
    if (!n) return;
    setActive((i) => firstEnabled(((i < 0 ? (dir === 1 ? -1 : 0) : i) + dir + n) % n, dir));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (!open) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openMenu();
      }
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
        e.preventDefault();
        setActive(firstEnabled(0, 1));
        break;
      case "End":
        e.preventDefault();
        setActive(firstEnabled(options.length - 1, -1));
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        choose(options[active]);
        break;
      case "Escape":
        // 不让外层弹窗收到 Esc
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        break;
      case "Tab":
        setOpen(false);
        break;
    }
  };

  // 键盘移动时把高亮项滚到可见
  useEffect(() => {
    if (!open || active < 0) return;
    const el = list.current?.querySelector<HTMLElement>(`[data-idx="${active}"]`);
    el?.scrollIntoView?.({ block: "nearest" });
  }, [open, active]);

  const optId = (i: number) => `${listId}-opt-${i}`;
  const Icon = current?.icon;

  return (
    <>
      <button
        ref={trigger}
        type="button"
        id={id}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open && active >= 0 ? optId(active) : undefined}
        aria-label={ariaLabel}
        aria-invalid={invalid || undefined}
        disabled={disabled}
        data-testid={testId}
        className={`ui-select tr ${size === "sm" ? "ui-sm" : ""} ${open ? "open" : ""} ${className ?? ""}`}
        style={style}
        onClick={() => (open ? setOpen(false) : openMenu())}
        onKeyDown={onKeyDown}
        // 空格的默认动作在 keyup 时触发 click，已在 keydown 里处理过
        onKeyUp={(e) => e.key === " " && e.preventDefault()}
      >
        {Icon && <Icon className="ui-select-ic" />}
        <span className={`ui-select-val ${mono ? "mono" : ""} ${current ? "" : "ph"}`}>{current?.label ?? placeholder}</span>
        <ChevronDown className="ui-select-chev" />
      </button>
      {open &&
        pos &&
        createPortal(
          <div ref={list} id={listId} role="listbox" aria-label={ariaLabel} className="ui-pop scroll" style={popStyle(pos)} data-placement={pos.placement}>
            {options.length === 0 && <div className="ui-opt-empty">没有可选项</div>}
            {options.map((o, i) => {
              const sel = o.value === value;
              const OIcon = o.icon;
              return (
                <div
                  key={o.value}
                  id={optId(i)}
                  role="option"
                  aria-selected={sel}
                  aria-disabled={o.disabled || undefined}
                  data-idx={i}
                  className={`ui-opt ${i === active ? "active" : ""} ${sel ? "sel" : ""}`}
                  onMouseEnter={() => !o.disabled && setActive(i)}
                  // 不抢走触发按钮的焦点
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => choose(o)}
                >
                  {OIcon && <OIcon className="ui-opt-ic" />}
                  <span className="ui-opt-body">
                    <span className={`ui-opt-label ${mono ? "mono" : ""}`}>{o.label}</span>
                    {o.description && <span className="ui-opt-desc">{o.description}</span>}
                  </span>
                  <Check className="ui-opt-check" style={{ visibility: sel ? "visible" : "hidden" }} />
                </div>
              );
            })}
          </div>,
          document.body,
        )}
    </>
  );
}
