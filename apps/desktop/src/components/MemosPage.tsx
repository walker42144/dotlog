import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import { IntegrationIcon } from "./icons";

interface MemosPageProps {
  onGoIntegrations: () => void;
}

export function MemosPage({ onGoIntegrations }: MemosPageProps) {
  const [instanceUrl, setInstanceUrl] = useState("");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let active = true;
    ipc
      .settingsGet("plugin:memos")
      .then((settings) => {
        if (!active) return;
        const url = settings?.instanceUrl;
        setInstanceUrl(typeof url === "string" ? url : "");
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  if (loading) {
    return (
      <div className="memos-empty">
        <p>载入中…</p>
      </div>
    );
  }

  if (!instanceUrl.trim()) {
    return (
      <div className="memos-empty">
        <IntegrationIcon size={36} />
        <h2>尚未配置 Memos 实例</h2>
        <p>请先在「集成」页面填写 Memos 实例地址与访问令牌。</p>
        <button
          className="ghost-btn"
          type="button"
          style={{ flex: "0 0 auto" }}
          onClick={onGoIntegrations}
        >
          前往集成配置
        </button>
      </div>
    );
  }

  return (
    <div className="memos-wrap">
      <header className="memos-head">
        <h2>Memos</h2>
        <a
          className="memos-link"
          href={instanceUrl}
          target="_blank"
          rel="noopener noreferrer"
          title="在浏览器中打开"
        >
          {instanceUrl}
        </a>
      </header>
      <iframe
        className="memos-frame"
        src={instanceUrl}
        title="Memos"
        referrerPolicy="no-referrer"
        allow="clipboard-read; clipboard-write"
      />
    </div>
  );
}
