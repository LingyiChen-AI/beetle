// 品牌图标：beetle-8（docs/logo/v4/beetle-8.svg 的副本，自带米色圆角底），标题栏、机器人头像、首次引导共用。
import logoUrl from "../assets/beetle-8.svg";

export default function BrandLogo({ size, className, round }: { size: number; className?: string; round?: boolean }) {
  return (
    <img
      src={logoUrl}
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      draggable={false}
      data-testid="brand-logo"
      className={className}
      style={{ display: "block", flex: "none", borderRadius: round ? "50%" : undefined }}
    />
  );
}
