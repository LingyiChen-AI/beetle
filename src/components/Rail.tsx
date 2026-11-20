// 56px 图标栏：机器人 / 统计 / 项目 / 数据源 / 技能中心 / 环境检查（异常时红色角标）/ 设置。
import { BarChart3, Bot, Database, FolderGit2, Puzzle, Settings, Stethoscope, type LucideIcon } from "lucide-react";

export type Page = "bots" | "stats" | "projects" | "datasources" | "skills" | "env" | "settings";

const ITEMS: { id: Page; label: string; Icon: LucideIcon }[] = [
  { id: "bots", label: "机器人", Icon: Bot },
  { id: "stats", label: "统计", Icon: BarChart3 },
  { id: "projects", label: "项目", Icon: FolderGit2 },
  { id: "datasources", label: "数据源", Icon: Database },
  { id: "skills", label: "技能中心", Icon: Puzzle },
  { id: "env", label: "环境检查", Icon: Stethoscope },
  { id: "settings", label: "设置", Icon: Settings },
];

export default function Rail({ page, envIssues, onGo }: { page: Page; envIssues: number; onGo: (p: Page) => void }) {
  return (
    <nav className="rail" aria-label="主导航">
      {ITEMS.map(({ id, label, Icon }) => {
        const n = id === "env" ? envIssues : 0;
        return (
          <button
            key={id}
            type="button"
            className={`rail-btn tr ${page === id ? "on" : ""}`}
            aria-label={n ? `${label}（${n} 项异常）` : label}
            aria-current={page === id ? "page" : undefined}
            onClick={() => onGo(id)}
          >
            <Icon style={{ width: 20, height: 20 }} />
            {n > 0 && (
              <span className="badge" data-testid="env-badge">
                {n}
              </span>
            )}
            <span className="tip">{n ? `${label} · ${n} 项需处理` : label}</span>
          </button>
        );
      })}
    </nav>
  );
}
