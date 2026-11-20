// echarts 封装（按需引入：柱状 / 折线 / 饼图 + 网格 / 提示 / 图例，canvas 渲染）。
// 容器尺寸变化时自动 resize；option 变化时整体替换（notMerge），深浅色 / 主题色切换由调用方重算 option。
// 测试里整个模块被 mock 掉（jsdom 没有 canvas）。
import { BarChart, LineChart, PieChart } from "echarts/charts";
import { GridComponent, LegendComponent, TooltipComponent } from "echarts/components";
import * as echarts from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import { useEffect, useRef } from "react";

echarts.use([BarChart, LineChart, PieChart, GridComponent, LegendComponent, TooltipComponent, CanvasRenderer]);

export type ChartOption = echarts.EChartsCoreOption;

export default function EChart({ option, height = 240, ariaLabel, testId }: { option: ChartOption; height?: number; ariaLabel: string; testId?: string }) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<ReturnType<typeof echarts.init> | null>(null);

  useEffect(() => {
    if (!el.current) return;
    const c = echarts.init(el.current, undefined, { renderer: "canvas" });
    chart.current = c;
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(() => c.resize()) : null;
    ro?.observe(el.current);
    return () => {
      ro?.disconnect();
      c.dispose();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    chart.current?.setOption(option, { notMerge: true, lazyUpdate: true });
  }, [option]);

  return <div ref={el} role="img" aria-label={ariaLabel} data-testid={testId} style={{ width: "100%", height }} />;
}
