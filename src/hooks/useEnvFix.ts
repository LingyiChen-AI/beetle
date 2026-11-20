// 环境项一键修复：调用 env_fix，实时收集 env-install-log，等 env-install-done 结束。
// 环境页与首次引导共用。「一键修复全部」按顺序逐项执行，上一项 done 之后才开始下一项。
import { useCallback, useEffect, useRef, useState } from "react";
import { envFix, errText, onEnvInstallDone, onEnvInstallLog, subscribe } from "../api";

export interface FixResult {
  ok: boolean;
  message: string;
}

export function useEnvFix(onDone?: () => void) {
  const [logs, setLogs] = useState<Record<string, string[]>>({});
  const [running, setRunning] = useState<Set<string>>(new Set());
  const [results, setResults] = useState<Record<string, FixResult>>({});
  const [batch, setBatch] = useState(false);
  const waiters = useRef(new Map<string, (r: FixResult) => void>());
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  useEffect(
    () =>
      subscribe(
        onEnvInstallLog((e) => setLogs((m) => ({ ...m, [e.id]: [...(m[e.id] ?? []), e.line].slice(-500) }))),
        onEnvInstallDone((e) => {
          const r = { ok: e.ok, message: e.message };
          setResults((m) => ({ ...m, [e.id]: r }));
          setRunning((s) => {
            const n = new Set(s);
            n.delete(e.id);
            return n;
          });
          const w = waiters.current.get(e.id);
          waiters.current.delete(e.id);
          w?.(r);
          onDoneRef.current?.();
        }),
      ),
    [],
  );

  /** 修复一项，等它的 env-install-done 才返回。 */
  const fix = useCallback(async (id: string): Promise<FixResult> => {
    setLogs((m) => ({ ...m, [id]: [] }));
    setResults((m) => {
      const n = { ...m };
      delete n[id];
      return n;
    });
    setRunning((s) => new Set(s).add(id));
    // 先登记等待，再调用命令：done 事件可能比 invoke 的返回先到
    const done = new Promise<FixResult>((resolve) => waiters.current.set(id, resolve));
    try {
      await envFix(id);
    } catch (e) {
      const r = { ok: false, message: errText(e) };
      waiters.current.delete(id);
      setRunning((s) => {
        const n = new Set(s);
        n.delete(id);
        return n;
      });
      setResults((m) => ({ ...m, [id]: r }));
      return r;
    }
    return done;
  }, []);

  /** 按顺序逐项修复。 */
  const fixAll = useCallback(
    async (ids: string[]) => {
      setBatch(true);
      try {
        for (const id of ids) await fix(id);
      } finally {
        setBatch(false);
      }
    },
    [fix],
  );

  return { logs, running, results, batch, fix, fixAll };
}
