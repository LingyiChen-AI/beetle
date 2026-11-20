// 开关类小控件：ToggleChip（可多选的标签）、ChipMultiSelect（一组 ToggleChip）、Checkbox、Segmented（单选分段）。
// 选中 / 未选中必须一眼分得清：ToggleChip 选中是主题色实线描边 + 浅色底 + 实心勾，未选中是灰色虚线描边 + 加号。
import { Check, Plus, type LucideIcon } from "lucide-react";
import { useRef, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";

export function ToggleChip({
  on,
  onChange,
  children,
  mono,
  icon: Icon,
  disabled,
  title,
  ariaLabel,
}: {
  on: boolean;
  onChange: (on: boolean) => void;
  children: ReactNode;
  mono?: boolean;
  icon?: LucideIcon;
  disabled?: boolean;
  title?: string;
  ariaLabel?: string;
}) {
  return (
    <button
      type="button"
      className={`tgl tr ${on ? "on" : ""}`}
      aria-pressed={on}
      aria-label={ariaLabel}
      disabled={disabled}
      title={title ?? (on ? "已选中，点击取消" : "点击选择")}
      onClick={() => onChange(!on)}
    >
      <span className="tgl-mark" aria-hidden>
        {on ? <Check /> : <Plus />}
      </span>
      {Icon && <Icon className="tgl-ic" />}
      <span className={mono ? "mono" : undefined}>{children}</span>
    </button>
  );
}

export interface ChipOption<T extends string> {
  value: T;
  label?: string;
  title?: string;
}

/** 多选标签组：点一下加入 / 移出，新加的排在后面。 */
export function ChipMultiSelect<T extends string>({
  options,
  value,
  onChange,
  mono,
  ariaLabel,
  empty,
  disabled,
}: {
  options: (T | ChipOption<T>)[];
  value: T[];
  onChange: (next: T[]) => void;
  mono?: boolean;
  ariaLabel?: string;
  /** 没有可选项时显示 */
  empty?: ReactNode;
  disabled?: boolean;
}) {
  const opts: ChipOption<T>[] = options.map((o) => (typeof o === "string" ? { value: o } : o));
  return (
    <div className="flex flex-wrap gap-1.5" role="group" aria-label={ariaLabel}>
      {opts.length === 0 && empty}
      {opts.map((o) => {
        const on = value.includes(o.value);
        return (
          <ToggleChip
            key={o.value}
            on={on}
            mono={mono}
            title={o.title}
            disabled={disabled}
            onChange={() => onChange(on ? value.filter((x) => x !== o.value) : [...value, o.value])}
          >
            {o.label ?? o.value}
          </ToggleChip>
        );
      })}
    </div>
  );
}

/** 复选框（button + role=checkbox）。variant=card 是整行可点的选项卡片，inline 是行内小勾选。 */
export function Checkbox({
  checked,
  onChange,
  label,
  description,
  ariaLabel,
  disabled,
  variant = "inline",
  right,
  className,
  style,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label?: ReactNode;
  description?: ReactNode;
  ariaLabel?: string;
  disabled?: boolean;
  variant?: "inline" | "card";
  /** 卡片右侧的附加内容（如状态说明） */
  right?: ReactNode;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      className={`${variant === "card" ? "opt ui-cbrow" : "ui-cb-inline"} tr ${checked ? "on" : ""} ${className ?? ""}`}
      style={style}
      onClick={() => onChange(!checked)}
    >
      <span className={`cb ${checked ? "on" : ""}`} aria-hidden>
        {checked && <Check />}
      </span>
      {(label != null || description != null) && (
        <span className="ui-cb-text">
          {label != null && <span className="ui-cb-label">{label}</span>}
          {description != null && <span className="ui-cb-desc">{description}</span>}
        </span>
      )}
      {right}
    </button>
  );
}

export interface SegOption<T extends string> {
  value: T;
  label: ReactNode;
  icon?: LucideIcon;
  title?: string;
}

/** 单选分段按钮（radiogroup）。←→ 切换。 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
  full,
  size = "sm",
  disabled,
  testId,
}: {
  value: T;
  options: SegOption<T>[];
  onChange: (value: T) => void;
  ariaLabel: string;
  /** 撑满父容器宽度、等分 */
  full?: boolean;
  size?: "sm" | "md";
  disabled?: boolean;
  testId?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const dir = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!dir || disabled) return;
    e.preventDefault();
    const i = options.findIndex((o) => o.value === value);
    const next = options[(i + dir + options.length) % options.length];
    onChange(next.value);
    ref.current?.querySelectorAll<HTMLButtonElement>("button")[options.indexOf(next)]?.focus();
  };
  return (
    <div ref={ref} className={`seg ${full ? "full" : ""} ${size === "md" ? "ui-md" : ""}`} role="radiogroup" aria-label={ariaLabel} data-testid={testId} onKeyDown={onKeyDown}>
      {options.map((o, idx) => {
        const on = o.value === value;
        const Icon = o.icon;
        const tabbable = on || (idx === 0 && !options.some((x) => x.value === value));
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={tabbable ? 0 : -1}
            title={o.title}
            disabled={disabled}
            className={`tr ${on ? "on" : ""}`}
            onClick={() => onChange(o.value)}
          >
            {Icon && <Icon />}
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
