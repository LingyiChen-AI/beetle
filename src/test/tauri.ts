// 测试里替换 @tauri-apps/api：invoke 记录调用并按命令名返回预设结果，listen 可由测试主动触发。
import { act } from "@testing-library/react";
import { vi } from "vitest";

type Handler = (args: Record<string, unknown>) => unknown;

export const invokeMock = vi.fn();
let handlers: Record<string, Handler> = {};
const listeners = new Map<string, Set<(e: { payload: unknown }) => void>>();

/** 设置各命令的返回值（函数则调用；返回 Promise.reject 可模拟后端报错）。 */
export function setHandlers(next: Record<string, Handler | unknown>) {
  handlers = {};
  for (const [name, h] of Object.entries(next)) {
    handlers[name] = typeof h === "function" ? (h as Handler) : () => h;
  }
}

invokeMock.mockImplementation(async (cmd: string, args: Record<string, unknown> = {}) => {
  const h = handlers[cmd];
  if (!h) throw new Error(`测试里没有给命令 ${cmd} 设置返回值`);
  return h(args);
});

export function resetTauri() {
  invokeMock.mockClear();
  handlers = {};
  listeners.clear();
}

/** 取某个命令所有调用的参数。 */
export function callsOf(cmd: string): Record<string, unknown>[] {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd).map((c) => (c[1] ?? {}) as Record<string, unknown>);
}

/** 模拟后端发出一个事件。 */
export function emitEvent(name: string, payload: unknown) {
  act(() => {
    listeners.get(name)?.forEach((cb) => cb({ payload }));
  });
}

export const coreMock = {
  invoke: (...a: unknown[]) => invokeMock(...a),
  Channel: class {
    onmessage: (m: unknown) => void = () => {};
  },
};

export const eventMock = {
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name)!.add(cb);
    return () => listeners.get(name)?.delete(cb);
  },
};

export const openerMock = { openUrl: vi.fn(async () => {}) };

/** @tauri-apps/api/window：记录最小化 / 最大化 / 关闭调用。 */
export const windowCalls = { minimize: vi.fn(async () => {}), toggleMaximize: vi.fn(async () => {}), close: vi.fn(async () => {}) };
export const windowMock = { getCurrentWindow: () => windowCalls };

/** @tauri-apps/api/app */
export const appMock = { getVersion: async () => "0.1.0" };
