// 输入框：TextInput（统一高度、焦点环、错误态、可带前置图标）、NumberInput（带 −/+ 步进，隐藏原生上下箭头）。
import { Minus, Plus, type LucideIcon } from "lucide-react";
import type { CSSProperties, InputHTMLAttributes, Ref } from "react";

type BaseInputProps = Omit<InputHTMLAttributes<HTMLInputElement>, "size">;

export function TextInput({
  inputSize = "md",
  mono,
  invalid,
  icon: Icon,
  className,
  ref,
  ...rest
}: BaseInputProps & {
  inputSize?: "sm" | "md";
  mono?: boolean;
  invalid?: boolean;
  icon?: LucideIcon;
  ref?: Ref<HTMLInputElement>;
}) {
  const input = (
    <input
      ref={ref}
      className={`inp ${inputSize === "sm" ? "ui-sm" : ""} ${mono ? "mono" : ""} ${className ?? ""}`}
      aria-invalid={invalid || undefined}
      {...rest}
    />
  );
  if (!Icon) return input;
  return (
    <span className="ui-input-wrap">
      <Icon aria-hidden />
      {input}
    </span>
  );
}

export function NumberInput({
  id,
  value,
  onChange,
  min,
  max,
  step = 1,
  ariaLabel,
  disabled,
  invalid,
  size = "md",
  width,
  style,
}: {
  id?: string;
  value: number | string;
  /** 原样给出输入框里的文字（可能是空串），由调用方决定怎么解析 */
  onChange: (raw: string) => void;
  min?: number;
  max?: number;
  step?: number;
  ariaLabel?: string;
  disabled?: boolean;
  invalid?: boolean;
  size?: "sm" | "md";
  width?: number | string;
  style?: CSSProperties;
}) {
  const num = value === "" ? NaN : Number(value);
  const clamp = (n: number) => Math.min(max ?? Number.POSITIVE_INFINITY, Math.max(min ?? Number.NEGATIVE_INFINITY, n));
  const bump = (d: number) => onChange(String(clamp((Number.isFinite(num) ? num : (min ?? 0)) + d)));
  const name = ariaLabel ?? "数值";
  return (
    <div className={`ui-num ${size === "sm" ? "ui-sm" : ""} ${invalid ? "invalid" : ""} ${disabled ? "disabled" : ""}`} style={{ width, ...style }}>
      <button type="button" tabIndex={-1} aria-label={`${name}减一`} disabled={disabled || (min != null && Number.isFinite(num) && num <= min)} onClick={() => bump(-step)}>
        <Minus />
      </button>
      <input
        id={id}
        type="number"
        inputMode="numeric"
        className="mono"
        value={value}
        min={min}
        max={max}
        step={step}
        aria-label={ariaLabel}
        aria-invalid={invalid || undefined}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
      <button type="button" tabIndex={-1} aria-label={`${name}加一`} disabled={disabled || (max != null && Number.isFinite(num) && num >= max)} onClick={() => bump(step)}>
        <Plus />
      </button>
    </div>
  );
}
