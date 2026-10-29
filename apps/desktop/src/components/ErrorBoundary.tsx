import { Component, type ErrorInfo, type ReactNode } from "react";
import { ipc } from "../lib/ipc";
export class ErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean; message: string }> {
  state = { failed: false, message: "" };
  static getDerivedStateFromError() { return { failed: true, message: "" }; }
  componentDidCatch(error: Error, info: ErrorInfo) { console.error("DotLog 界面错误", error.name, info.componentStack); }
  render() {
    if (!this.state.failed) return this.props.children;
    return <main className="settings-page" role="alert"><h1>界面遇到问题</h1><p>重新载入界面会重新读取本地记录，不会清空数据库。</p><button className="primary-btn" onClick={() => window.location.reload()}>重新载入界面</button><button className="ghost-btn" onClick={() => { void ipc.diagnosticsSnapshot().then((r) => this.setState({ message: `诊断包已保存：${r.path}` })).catch(() => this.setState({ message: "诊断包生成失败，请保留数据目录后再重试。" })); }}>保存诊断包</button><p>{this.state.message}</p></main>;
  }
}
