// 自动更新：顶部提示条（发现新版本时出现，可「立即更新」或「稍后」），以及设置页「关于」里用的 useUpdate。
// 后端启动后检查一次、之后每 30 分钟检查一次，发现新版本推 update-available 事件；这里只负责展示和触发安装。
import { Download, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { checkUpdate, errText, installUpdate, onUpdateAvailable, updateStatus, type UpdateStatus } from "../api";

export function useUpdate() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState<"check" | "install" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    updateStatus()
      .then((s) => alive && setStatus(s))
      .catch(() => {});
    const un = onUpdateAvailable((s) => alive && setStatus(s));
    return () => {
      alive = false;
      void un.then((f) => f());
    };
  }, []);

  const check = useCallback(async () => {
    setError(null);
    setBusy("check");
    try {
      setStatus(await checkUpdate());
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  }, []);

  const install = useCallback(async () => {
    setError(null);
    setBusy("install");
    try {
      await installUpdate(); // 成功后客户端会重启
    } catch (e) {
      setError(errText(e));
      setBusy(null);
    }
  }, []);

  return { status, busy, error, check, install };
}

export default function UpdateBanner() {
  const { status, busy, error, install } = useUpdate();
  const [dismissed, setDismissed] = useState<string | null>(null);
  const version = status?.available;
  if (!version || dismissed === version) return null;
  return (
    <div className="update-banner" role="status" data-testid="update-banner">
      <RefreshCw className="update-banner-icon" />
      <div className="min-w-0 flex-1">
        <div>
          发现新版本 <span className="mono">v{version}</span>
          <span style={{ color: "var(--muted)" }}>（当前 v{status?.current}）</span>
        </div>
        {(error || status?.notes) && (
          <div className="update-banner-notes" style={error ? { color: "var(--danger-fg)" } : undefined}>
            {error ?? status?.notes}
          </div>
        )}
      </div>
      <button type="button" className="btn btn-pri btn-sm tr" disabled={busy === "install"} onClick={() => void install()}>
        <Download />
        {busy === "install" ? "正在下载安装…" : "立即更新"}
      </button>
      <button type="button" className="btn btn-ghost btn-sm tr" aria-label="稍后更新" disabled={busy === "install"} onClick={() => setDismissed(version)}>
        <X />
        稍后
      </button>
    </div>
  );
}
