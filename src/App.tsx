// 客户端外壳：自绘标题栏 + 56px 图标栏 + 240px 机器人列表 + 工作台（照定稿原型 option-2-final.html）。
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  envCheck,
  envRequirements,
  errText,
  isDevMock,
  listBots,
  onBotState,
  onEnvInstallDone,
  onRunUpdated,
  setStartWithClient,
  startBot,
  stopBot,
  subscribe,
  type BotView,
  type EnvItem,
  type EnvRequirements,
} from "./api";
import { botBlockers } from "./envRules";
import BotList from "./components/BotList";
import { ToastProvider, useToast, type BotKind } from "./components/common";
import NewBotWizard from "./components/wizard/NewBotWizard";
import Onboarding from "./components/wizard/Onboarding";
import DataSourcesPage from "./components/pages/DataSourcesPage";
import EnvPage from "./components/pages/EnvPage";
import ProjectsPage from "./components/pages/ProjectsPage";
import SkillsPage from "./components/pages/SkillsPage";
import SettingsPage from "./components/pages/SettingsPage";
import Rail, { type Page } from "./components/Rail";
import TitleBar from "./components/TitleBar";
import UpdateBanner from "./components/UpdateBanner";
import Workbench, { type Tab } from "./components/Workbench";
import { AppearanceProvider } from "./theme/appearance";
import { EmptyState, LoadingState } from "./components/ui";
// 数据统计页带 echarts（约 600 KB），用到时再加载，不打进主包
const StatsPage = lazy(() => import("./components/pages/StatsPage"));
import { Bot as BotGlyph, Plus } from "lucide-react";

const TABS: Tab[] = ["overview", "groups", "skills", "logs", "config"];
const PAGES: Page[] = ["bots", "stats", "projects", "datasources", "skills", "env", "settings"];

/** 开发 mock 下可用 #page=bots&bot=b1&tab=logs&run=r0 直接打开某个视图（方便截图）。 */
function devInitialView(): { page?: Page; bot?: string; tab?: Tab; run?: string } {
  if (!isDevMock()) return {};
  const q = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  const page = q.get("page") as Page | null;
  const tab = q.get("tab") as Tab | null;
  return {
    page: page && PAGES.includes(page) ? page : undefined,
    bot: q.get("bot") ?? undefined,
    tab: tab && TABS.includes(tab) ? tab : undefined,
    run: q.get("run") ?? undefined,
  };
}

function Shell() {
  const toast = useToast();
  const init = useMemo(devInitialView, []);
  const [page, setPage] = useState<Page>(init.page ?? "bots");
  const [views, setViews] = useState<BotView[]>([]);
  const [botId, setBotId] = useState<string | null>(init.bot ?? null);
  const [tab, setTab] = useState<Tab>(init.tab ?? "overview");
  const [runId, setRunId] = useState<string | null>(init.run ?? null);
  const [stopping, setStopping] = useState<Set<string>>(new Set());
  const [since, setSince] = useState<Record<string, number>>({});
  const [env, setEnv] = useState<EnvItem[]>([]);
  /** 每个机器人的环境阻断规则（后端 env_requirements，随机器人配置变化，跟着 reload 刷新） */
  const [req, setReq] = useState<EnvRequirements | null>(null);
  const [adding, setAdding] = useState(false);
  const [onboarding, setOnboarding] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** 首次加载：没有任何机器人时自动进入首次引导 */
  const firstLoad = useRef(true);

  const reload = useCallback(async () => {
    // 规则取失败时沿用上一次的（取不到就不在前端拦，交给后端 start_bot 判断）
    envRequirements()
      .then(setReq)
      .catch(() => {});
    try {
      const v = await listBots();
      setViews(v);
      setLoadError(null);
      if (firstLoad.current) {
        firstLoad.current = false;
        if (v.length === 0) setOnboarding(true);
      }
      setBotId((cur) => (cur && v.some((x) => x.bot.id === cur) ? cur : (v[0]?.bot.id ?? null)));
    } catch (e) {
      setLoadError(errText(e));
    }
  }, []);

  /** 只刷新 alive（不动状态，避免盖掉更新的 bot-state 事件）。 */
  const refreshAlive = useCallback(async () => {
    try {
      const fresh = await listBots();
      setViews((prev) => prev.map((v) => ({ ...v, alive: fresh.find((f) => f.bot.id === v.bot.id)?.alive ?? v.alive })));
    } catch {
      // 读不到就保持原样
    }
  }, []);

  const [checking, setChecking] = useState(false);
  const checkEnv = useCallback(async () => {
    setChecking(true);
    try {
      setEnv(await envCheck());
    } catch {
      // 检查失败：不阻塞启动，环境页再细看
      setEnv([]);
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void reload();
    void checkEnv();
    return subscribe(
      onBotState((e) => {
        setViews((prev) => prev.map((v) => (v.bot.id === e.bot_id ? { ...v, state: e.state } : v)));
        setStopping((s) => {
          if (!s.has(e.bot_id) || e.state.kind === "running") return s;
          const n = new Set(s);
          n.delete(e.bot_id);
          return n;
        });
        if (e.state.kind === "running") setSince((m) => ({ ...m, [e.bot_id]: Date.now() }));
        // 异常时连接任务可能还在重试（alive），也可能已结束：重新取一次 alive，决定显示「停止」还是「启动」
        if (e.state.kind === "error") void refreshAlive();
      }),
      // 运行记录变化时刷新「今日运行数」
      onRunUpdated(() => void reload()),
      onEnvInstallDone(() => void checkEnv()),
    );
  }, [reload, checkEnv, refreshAlive]);

  const blockersOf = useCallback((id: string) => botBlockers(env, req, id), [env, req]);
  const envIssues = env.filter((e) => e.status === "error" || e.status === "warn").length;
  const kindOf = useCallback((v: BotView): BotKind => (stopping.has(v.bot.id) ? "stopping" : v.state.kind), [stopping]);
  const index = views.findIndex((v) => v.bot.id === botId);
  const view = index >= 0 ? views[index] : null;

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn();
    } catch (e) {
      toast(errText(e), "err");
    }
  };

  const start = (id: string) =>
    act(async () => {
      if (blockersOf(id).length) throw "环境检查未通过，无法启动 · 先去修复";
      setViews((prev) => prev.map((v) => (v.bot.id === id ? { ...v, state: { kind: "connecting" } } : v)));
      try {
        await startBot(id);
      } catch (e) {
        await reload();
        throw e;
      }
      // 乐观改成的「连接中」用后端真实状态校正：界面还停在这个乐观值时才覆盖（不盖掉期间收到的 bot-state 事件），
      // 后端什么都没做时也不会一直卡在「连接中…」。
      try {
        const fresh = (await listBots()).find((f) => f.bot.id === id);
        if (fresh)
          setViews((prev) =>
            prev.map((v) => (v.bot.id === id && v.state.kind === "connecting" ? { ...v, state: fresh.state, alive: fresh.alive } : v)),
          );
      } catch {
        // 读不到就等事件
      }
    });
  const stop = (id: string) =>
    act(async () => {
      setStopping((s) => new Set(s).add(id));
      try {
        await stopBot(id);
      } catch (e) {
        setStopping((s) => {
          const n = new Set(s);
          n.delete(id);
          return n;
        });
        throw e;
      }
    });

  const go = (p: Page) => {
    setPage(p);
    if (p === "bots") void checkEnv();
  };

  return (
    <div className="app">
      <TitleBar onOpenSettings={() => setPage("settings")} />
      <div className="app-body">
        <Rail page={page} envIssues={envIssues} onGo={go} />
        {page === "bots" && (
          <BotList
            views={views}
            kindOf={kindOf}
            selected={botId}
            blockedOf={(id) => blockersOf(id).length > 0}
            onSelect={(id) => {
              if (id !== botId) {
                setTab("overview");
                setRunId(null);
              }
              setBotId(id);
            }}
            onGoEnv={() => setPage("env")}
            onAdd={() => setAdding(true)}
          />
        )}
        <main className="main">
          <UpdateBanner />
          {loadError && (
            <div className="alert-err" role="alert" style={{ margin: "12px 24px 0" }}>
              读取机器人失败：{loadError}
            </div>
          )}
          {page === "bots" &&
            (view ? (
              <Workbench
                key={view.bot.id}
                view={view}
                index={index}
                kind={kindOf(view)}
                runningSince={since[view.bot.id] ?? null}
                envErrors={blockersOf(view.bot.id)}
                tab={tab}
                runId={runId}
                onTab={setTab}
                onOpenRun={(id) => {
                  setRunId(id);
                  setTab("logs");
                }}
                onStart={() => void start(view.bot.id)}
                onStop={() => void stop(view.bot.id)}
                onToggleAutostart={(on) =>
                  void act(async () => {
                    await setStartWithClient(view.bot.id, on);
                    await reload();
                    toast(on ? "已开启：客户端启动时自动启动该机器人" : "已关闭随客户端启动");
                  })
                }
                onGoEnv={() => setPage("env")}
                onGoProjects={() => setPage("projects")}
                onGoSkills={() => setPage("skills")}
                onChanged={reload}
                onDeleted={() => {
                  setBotId(null);
                  void reload();
                }}
              />
            ) : (
              <div className="grid flex-1 place-items-center">
                {views.length ? (
                  <EmptyState icon={BotGlyph} title="选择左侧的机器人" desc="查看它的运行概览、所在群、技能和运行日志" />
                ) : (
                  <EmptyState
                    icon={BotGlyph}
                    title="还没有机器人"
                    desc="接入一个飞书应用，把它拉进群里 @ 一下就能开始查 bug"
                    action={
                      <button type="button" className="btn btn-pri tr" onClick={() => setAdding(true)}>
                        <Plus />
                        创建第一个机器人
                      </button>
                    }
                  />
                )}
              </div>
            ))}
          {page === "stats" && (
            <Suspense fallback={<LoadingState text="正在加载数据统计…" />}>
              <StatsPage views={views} />
            </Suspense>
          )}
          {page === "projects" && <ProjectsPage onGoDataSources={() => setPage("datasources")} />}
          {page === "datasources" && <DataSourcesPage />}
          {page === "skills" && <SkillsPage env={env} />}
          {page === "env" && <EnvPage env={env} checking={checking} onRecheck={checkEnv} />}
          {page === "settings" && <SettingsPage onOpenOnboarding={() => setOnboarding(true)} />}
        </main>
      </div>
      {onboarding && (
        <Onboarding
          env={env}
          requirements={req}
          checking={checking}
          onRecheck={checkEnv}
          onChanged={reload}
          onClose={() => {
            setOnboarding(false);
            setPage("bots");
            void reload();
          }}
          onCreateBot={() => {
            setOnboarding(false);
            setAdding(true);
          }}
        />
      )}
      {adding && (
        <NewBotWizard
          onClose={() => setAdding(false)}
          onGoProjects={() => {
            setAdding(false);
            setPage("projects");
          }}
          onCreated={(bot, started) => {
            setAdding(false);
            setBotId(bot.id);
            setTab("overview");
            setPage("bots");
            void reload();
            toast(started ? `已创建「${bot.name}」，正在启动` : `已创建「${bot.name}」`);
          }}
        />
      )}
    </div>
  );
}

export default function App() {
  return (
    <AppearanceProvider>
      <ToastProvider>
        <Shell />
      </ToastProvider>
    </AppearanceProvider>
  );
}
