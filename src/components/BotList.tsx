// 240px 机器人列表。
import { MessageCircleQuestion, Plus, TriangleAlert } from "lucide-react";
import type { Bot, BotView } from "../api";
import BrandLogo from "./BrandLogo";
import { BOT_META, type BotKind } from "./common";

/** 头像配色按序取（都是语义 token）。 */
const AVATAR = [
  { bg: "var(--accent)", ink: "var(--on-accent)" },
  { bg: "var(--info)", ink: "var(--on-info)" },
  { bg: "var(--warning)", ink: "var(--on-warning)" },
  { bg: "var(--success)", ink: "var(--on-accent)" },
];
const avatarColor = (index: number) => AVATAR[Math.max(0, index) % AVATAR.length];
/** 查 bug 类机器人的头像就是品牌图标 beetle-8（自带底色），不再叠一层配色底；答疑类用配色底 + 图标。 */
const usesBrandLogo = (bot: Bot) => bot.template !== "qa";
export const avatarOf = (index: number, bot?: Bot) =>
  bot && usesBrandLogo(bot) ? { bg: "transparent", ink: "inherit" } : avatarColor(index);
/** `size` 是图标尺寸，头像框是它的两倍（列表 17 → 34px，工作台 23 → 46px）；品牌图标直接铺满头像框。 */
export const BotIcon = ({ bot, size }: { bot: Bot; size: number }) =>
  usesBrandLogo(bot) ? <BrandLogo size={size * 2} /> : <MessageCircleQuestion style={{ width: size, height: size }} />;
export const engineText = (e: Bot["engine"]) => (e === "claude" ? "Claude Code" : "Codex");

export default function BotList({
  views,
  kindOf,
  selected,
  blockedOf,
  onSelect,
  onGoEnv,
  onAdd,
}: {
  views: BotView[];
  kindOf: (v: BotView) => BotKind;
  selected: string | null;
  /** 这个机器人是否被环境检查阻断（按它自己的所需项算，见 src/envRules.ts） */
  blockedOf: (botId: string) => boolean;
  onSelect: (id: string) => void;
  onGoEnv: () => void;
  onAdd: () => void;
}) {
  return (
    <aside className="botlist" aria-label="机器人列表">
      <div className="bl-head">
        <h2>
          机器人 <span style={{ color: "var(--dim)", fontWeight: 400 }}>{views.length}</span>
        </h2>
      </div>
      <div className="scroll flex-1" style={{ padding: 8 }}>
        <div style={{ fontSize: 11, color: "var(--dim)", padding: 6, letterSpacing: ".04em" }}>我的机器人</div>
        {views.length === 0 && (
          <div style={{ padding: "18px 8px", color: "var(--muted)", fontSize: 12 }}>还没有机器人，点下方「新增机器人」。</div>
        )}
        {views.map((v, i) => {
          const kind = kindOf(v);
          const m = BOT_META[kind];
          const av = avatarOf(i, v.bot);
          return (
            <div key={v.bot.id} style={{ marginBottom: 4 }}>
              <button
                type="button"
                className={`bot-row tr ${selected === v.bot.id ? "on" : ""}`}
                aria-current={selected === v.bot.id ? "true" : undefined}
                onClick={() => onSelect(v.bot.id)}
              >
                <div className="avatar" style={{ background: av.bg, color: av.ink }}>
                  <BotIcon bot={v.bot} size={17} />
                  <span className={`sd ${m.dot}`} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="truncate" style={{ fontWeight: 600 }}>
                    {v.bot.name}
                  </div>
                  <div className="flex items-center gap-1.5" style={{ fontSize: 12, color: "var(--muted)", marginTop: 2 }}>
                    <span>{m.text}</span>
                    <span style={{ color: "var(--dim)" }}>·</span>
                    <span>{engineText(v.bot.engine)}</span>
                  </div>
                </div>
                <div style={{ textAlign: "right" }}>
                  <div className="mono" style={{ fontWeight: 600, fontSize: 13, color: v.today_runs ? undefined : "var(--dim)" }}>
                    {v.today_runs}
                  </div>
                  <div style={{ fontSize: 10, color: "var(--dim)" }}>今日</div>
                </div>
              </button>
              {blockedOf(v.bot.id) && kind !== "running" && (
                <button type="button" className="hint-err tr" onClick={onGoEnv}>
                  <TriangleAlert />
                  环境异常，无法启动 · 去修复
                </button>
              )}
            </div>
          );
        })}
      </div>
      <div style={{ padding: 10, borderTop: "1px solid var(--line)" }}>
        <button
          type="button"
          className="btn tr w-full justify-center"
          style={{ borderStyle: "dashed", background: "transparent" }}
          onClick={onAdd}
        >
          <Plus />
          新增机器人
        </button>
      </div>
    </aside>
  );
}
