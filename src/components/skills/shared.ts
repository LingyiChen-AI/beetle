// 技能中心卡片与详情弹窗共用：图标、依赖名、安装位置名、环境判断，以及更新 / 卸载操作。
import { Bug, Puzzle, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { catalogInstall, catalogUninstall, errText, SKILL_TARGETS, type CatalogEntry, type EnvItem, type SkillTarget } from "../../api";
import { useToast } from "../common";

const SKILL_ICON: Record<string, LucideIcon> = { "bug-hunt": Bug };
export const skillIcon = (name: string): LucideIcon => SKILL_ICON[name] ?? Puzzle;

export const REQUIRE_LABEL: Record<string, string> = { python: "Python", node: "Node", lark_cli: "lark-cli", git: "Git", ocr: "ocr" };
export const targetLabel = (t: SkillTarget) => SKILL_TARGETS.find((x) => x.id === t)?.label ?? t;

/** 环境检查里这个 CLI 是否已装（ok / warn 都算装了，warn 一般是没登录）。 */
export const detected = (env: EnvItem[], id: string) => env.some((e) => e.id === id && (e.status === "ok" || e.status === "warn"));
export const missing = (env: EnvItem[], id: string) => env.some((e) => e.id === id && e.status === "error");

/** 更新 / 卸载（按安装位置）：进行中禁用按钮，失败的错误就近显示，成功后 toast 并刷新列表。 */
export function useSkillActions(entry: CatalogEntry, onChanged: () => Promise<void> | void) {
  const toast = useToast();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (fn: () => Promise<void>, ok: string) => {
    setError(null);
    setBusy(true);
    try {
      await fn();
      toast(ok);
      await onChanged();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const update = (t: SkillTarget) => run(() => catalogInstall(entry.name, [t]), `已把「${entry.display_name}」更新到 v${entry.version}（${targetLabel(t)}）`);
  const uninstall = (t: SkillTarget) => {
    if (!window.confirm(`从 ${targetLabel(t)} 卸载「${entry.display_name}」？`)) return;
    void run(() => catalogUninstall(entry.name, t), `已从 ${targetLabel(t)} 卸载「${entry.display_name}」`);
  };
  return { error, busy, update, uninstall };
}

/** 1.2 KB / 512 B */
export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
